use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    rc::Rc,
};

use anyhow::{bail, Context, Result};

mod model;
mod parser;

use model::LoadCommand;

use crate::compressor::{
    compress_config::ModelConfig,
    model::{HashTable, Model, NOrderByteData},
    model_finder::create_default_model_config,
    Encoder,
};

const DEFAULT_NORDER_TABLE_POW2: u32 = 26;
const NORDER_RECORD_BYTES: usize = 4;
const MIXER_CONTEXT_ROWS: usize = 256 * 255;

const BOOTSTRAP_S: &str = include_str!("macho/template/bootstrap.s");
const DECODER_S: &str = include_str!("macho/template/decoder.s");
const MODEL_SUPPORT_S: &str = include_str!("macho/template/model_support.s");
const NORDER_BYTE_S: &str = include_str!("macho/template/norder_byte.s");
const WORD_S: &str = include_str!("macho/template/word.s");
const LN_MIXER_S: &str = include_str!("macho/template/ln_mixer.s");

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Input Mach-O binary executable to compress
    #[arg(short, long)]
    pub input: PathBuf,

    /// Output directory
    #[arg(short, long)]
    pub output_directory: String,
}

pub fn run(args: Args) -> Result<()> {
    let binary = fs::read(&args.input)
        .with_context(|| format!("Failed to read {}", args.input.display()))?;

    let model_config = create_default_model_config();
    let model = model_config
        .create_model(Rc::new(RefCell::new(HashTable::<NOrderByteData>::new(
            DEFAULT_NORDER_TABLE_POW2,
        ))))
        .context("Failed to create compression model")?;
    let compressed_macho = compress_binary_with_model(&binary, model)?;
    let total_uncompressed = compressed_macho.uncompressed.len();

    println!(
        "Found {} segment(s) to compress ({} bytes total):",
        compressed_macho.segments.len(),
        total_uncompressed
    );
    for segment in &compressed_macho.segments {
        println!("  {:<16} {} bytes", segment.name, segment.size);
    }

    let output_dir = PathBuf::from(&args.output_directory);
    fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "Failed to create output directory: {}",
            output_dir.display()
        )
    })?;

    let out_path = output_dir.join("compressed.bin");
    fs::write(&out_path, &compressed_macho.compressed)
        .with_context(|| format!("Failed to write {}", out_path.display()))?;

    let decompressor_path = build_decompressor(
        &output_dir,
        &model_config,
        &out_path,
        compressed_macho.uncompressed.len(),
    )
    .context("Failed to build Mach-O decompressor")?;

    println!(
        "Compressed {} bytes -> {} bytes ({:.1}% of original)",
        total_uncompressed,
        compressed_macho.compressed.len(),
        100.0 * compressed_macho.compressed.len() as f64 / total_uncompressed as f64,
    );
    println!("Output written to {}", out_path.display());
    println!("Decompressor written to {}", decompressor_path.display());

    Ok(())
}

fn build_decompressor(
    output_dir: &Path,
    model_config: &ModelConfig,
    compressed_path: &Path,
    output_len: usize,
) -> Result<PathBuf> {
    if !command_available("clang") {
        bail!("clang is required to build the Mach-O decompressor");
    }

    let build_dir = output_dir.join("build");
    fs::create_dir_all(&build_dir)
        .with_context(|| format!("Failed to create {}", build_dir.display()))?;

    let sources = [
        ("bootstrap.s", BOOTSTRAP_S.to_owned()),
        ("decoder.s", DECODER_S.to_owned()),
        ("model_support.s", MODEL_SUPPORT_S.to_owned()),
        ("norder_byte.s", NORDER_BYTE_S.to_owned()),
        ("word.s", WORD_S.to_owned()),
        ("ln_mixer.s", LN_MIXER_S.to_owned()),
        (
            "payload.s",
            render_payload_assembly(compressed_path, output_len),
        ),
        (
            "model.s",
            render_model_assembly(model_config, DEFAULT_NORDER_TABLE_POW2)?,
        ),
        ("after_decode.c", render_after_decode_c()),
    ];

    let mut source_paths = Vec::with_capacity(sources.len());
    for (name, src) in sources {
        let path = build_dir.join(name);
        fs::write(&path, src).with_context(|| format!("Failed to write {}", path.display()))?;
        source_paths.push(path);
    }

    let decompressor_path = output_dir.join("decompressor");
    let mut command = Command::new("clang");
    command.arg("-arch").arg("arm64");
    for path in &source_paths {
        command.arg(path);
    }
    command.arg("-o").arg(&decompressor_path);

    let output = command.output().context("Failed to run clang")?;
    assert_command_success("build Mach-O decompressor", &output)?;

    Ok(decompressor_path)
}

fn render_payload_assembly(compressed_path: &Path, output_len: usize) -> String {
    format!(
        r#".section __DATA,__const
.p2align 3
.globl _websqz_compressed_start
_websqz_compressed_start:
.incbin "{compressed_path}"
.globl _websqz_compressed_end
_websqz_compressed_end:

.section __DATA,__bss
.p2align 3
.globl _websqz_output_start
_websqz_output_start:
.space {output_len}
.globl _websqz_output_end
_websqz_output_end:
"#,
        compressed_path = escape_assembly_path(compressed_path),
    )
}

fn render_after_decode_c() -> String {
    r#"#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

void websqz_after_decode(uint8_t *output, uint64_t len, int argc, char **argv) {
    const char *path = argc > 1 ? argv[1] : "decoded.bin";
    FILE *file = fopen(path, "wb");
    if (!file) {
        fprintf(stderr, "failed to open %s: %s\n", path, strerror(errno));
        exit(1);
    }

    size_t written = fwrite(output, 1, (size_t)len, file);
    if (written != (size_t)len) {
        fprintf(stderr, "failed to write %s: wrote %zu of %llu bytes\n",
                path, written, (unsigned long long)len);
        fclose(file);
        exit(1);
    }

    if (fclose(file) != 0) {
        fprintf(stderr, "failed to close %s: %s\n", path, strerror(errno));
        exit(1);
    }
}
"#
    .to_owned()
}

fn render_model_assembly(model_config: &ModelConfig, table_pow2: u32) -> Result<String> {
    let mut generator = ModelAssemblyGenerator::new(table_pow2);
    let root = generator.render_model(model_config, "_websqz_model_ctx")?;
    Ok(generator.finish(root))
}

struct ModelAssemblyGenerator {
    src: String,
    next_id: usize,
    table_pow2: u32,
    uses_norder_table: bool,
}

struct RenderedModel {
    ctx_symbol: String,
    pred_symbol: &'static str,
    learn_symbol: &'static str,
}

impl ModelAssemblyGenerator {
    fn new(table_pow2: u32) -> Self {
        Self {
            src: String::new(),
            next_id: 0,
            table_pow2,
            uses_norder_table: false,
        }
    }

    fn finish(mut self, root: RenderedModel) -> String {
        self.src.push_str(&format!(
            r#"
.text
.align 2
.globl _websqz_model_predict
_websqz_model_predict:
    stp     x29, x30, [sp, #-16]!
    mov     x29, sp
    bl      {pred}
    bl      _websqz_prob_squash
    ldp     x29, x30, [sp], #16
    ret

.globl _websqz_model_learn
_websqz_model_learn:
    b       {learn}
"#,
            pred = root.pred_symbol,
            learn = root.learn_symbol,
        ));

        if self.uses_norder_table {
            let table_bytes = (1usize << self.table_pow2) * NORDER_RECORD_BYTES;
            self.src.push_str(&format!(
                r#"
.zerofill __DATA,__bss,_websqz_norder_table,{table_bytes},2
"#
            ));
        }

        self.src
    }

    fn render_model(
        &mut self,
        model_config: &ModelConfig,
        ctx_symbol: &str,
    ) -> Result<RenderedModel> {
        match model_config {
            ModelConfig::NOrderByte { byte_mask } => {
                let byte_mask = u8::from_str_radix(byte_mask.trim_start_matches("0b"), 2)
                    .with_context(|| format!("Invalid NOrderByte mask {byte_mask}"))?;
                self.render_norder_model(ctx_symbol, byte_mask, false)
            }
            ModelConfig::Word => self.render_norder_model(ctx_symbol, 0, true),
            ModelConfig::Mixer { models } => self.render_mixer_model(ctx_symbol, models),
            ModelConfig::AdaptiveProbabilityMap(_) => {
                bail!("Mach-O assembly decompressor does not support AdaptiveProbabilityMap yet")
            }
        }
    }

    fn render_norder_model(
        &mut self,
        ctx_symbol: &str,
        byte_mask: u8,
        is_word: bool,
    ) -> Result<RenderedModel> {
        self.uses_norder_table = true;
        let table_len = 1usize << self.table_pow2;
        let (magic_num, prev_bytes, mask, pred_symbol, learn_symbol, is_word_flag) = if is_word {
            (
                model_hash(1337, 2),
                2166136261u64,
                u64::MAX,
                "_websqz_word_predict",
                "_websqz_word_learn",
                1u32,
            )
        } else {
            (
                model_hash(byte_mask as u32, 2),
                0u64,
                byte_mask_to_context_mask(byte_mask),
                "_websqz_norder_byte_predict",
                "_websqz_norder_byte_learn",
                0u32,
            )
        };

        self.src.push_str(".section __DATA,__data\n.p2align 3\n");
        if ctx_symbol.starts_with('_') {
            self.src.push_str(&format!(".globl {ctx_symbol}\n"));
        }
        self.src.push_str(&format!(
            r#"{ctx_symbol}:
    .long 0
    .long 1
    .long 0x{magic_num:08x}
    .long 15
    .quad 0x{prev_bytes:016x}
    .quad 0x{mask:016x}
    .long {is_word_flag}
    .long 0
    .quad _websqz_norder_table
    .quad {hash_mask}
"#,
            hash_mask = table_len - 1,
        ));

        Ok(RenderedModel {
            ctx_symbol: ctx_symbol.to_owned(),
            pred_symbol,
            learn_symbol,
        })
    }

    fn render_mixer_model(
        &mut self,
        ctx_symbol: &str,
        models: &[ModelConfig],
    ) -> Result<RenderedModel> {
        if models.is_empty() {
            bail!("Mach-O assembly decompressor cannot render an empty mixer")
        }

        let mixer_id = self.alloc_id();
        let child_ctxs_symbol = format!("L_websqz_mixer_{mixer_id}_child_contexts");
        let pred_fns_symbol = format!("L_websqz_mixer_{mixer_id}_predict_fns");
        let learn_fns_symbol = format!("L_websqz_mixer_{mixer_id}_learn_fns");
        let base_weights_symbol = format!("L_websqz_mixer_{mixer_id}_base_weights");
        let ctx_weights_symbol = format!("L_websqz_mixer_{mixer_id}_ctx_weights");
        let ctx_init_symbol = format!("L_websqz_mixer_{mixer_id}_ctx_init");
        let last_p_symbol = format!("L_websqz_mixer_{mixer_id}_last_p");

        let mut children = Vec::with_capacity(models.len());
        for model in models {
            let child_symbol = format!("L_websqz_mixer_{mixer_id}_child_{}", children.len());
            children.push(self.render_model(model, &child_symbol)?);
        }

        let num_models = children.len();
        let ctx_weight_bytes = MIXER_CONTEXT_ROWS * num_models * size_of::<f64>();
        let initial_weight = 1.0 / num_models as f64;

        self.src.push_str(".section __DATA,__data\n.p2align 3\n");
        if ctx_symbol.starts_with('_') {
            self.src.push_str(&format!(".globl {ctx_symbol}\n"));
        }
        self.src.push_str(&format!(
            r#"{ctx_symbol}:
    .long {num_models}
    .long 1
    .long 0
    .long 0
    .double 0.0
    .quad {child_ctxs_symbol}
    .quad {pred_fns_symbol}
    .quad {learn_fns_symbol}
    .quad {base_weights_symbol}
    .quad {ctx_weights_symbol}
    .quad {ctx_init_symbol}
    .quad {last_p_symbol}

.p2align 3
{child_ctxs_symbol}:
"#
        ));
        for child in &children {
            self.src
                .push_str(&format!("    .quad {}\n", child.ctx_symbol));
        }

        self.src.push_str(&format!("{pred_fns_symbol}:\n"));
        for child in &children {
            self.src
                .push_str(&format!("    .quad {}\n", child.pred_symbol));
        }

        self.src.push_str(&format!("{learn_fns_symbol}:\n"));
        for child in &children {
            self.src
                .push_str(&format!("    .quad {}\n", child.learn_symbol));
        }

        self.src.push_str(&format!("{base_weights_symbol}:\n"));
        for _ in &children {
            self.src
                .push_str(&format!("    .double {:.17}\n", initial_weight));
        }

        self.src.push_str(&format!(
            r#"{last_p_symbol}:
    .space {last_p_bytes}

.section __DATA,__bss
.p2align 3
{ctx_init_symbol}:
    .space {ctx_init_bytes}
.p2align 3
{ctx_weights_symbol}:
    .space {ctx_weight_bytes}
"#,
            last_p_bytes = num_models * size_of::<f64>(),
            ctx_init_bytes = MIXER_CONTEXT_ROWS,
        ));

        Ok(RenderedModel {
            ctx_symbol: ctx_symbol.to_owned(),
            pred_symbol: "_websqz_ln_mixer_predict_stretched",
            learn_symbol: "_websqz_ln_mixer_learn",
        })
    }

    fn alloc_id(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

fn model_hash(mut value: u32, shift: u32) -> u32 {
    value ^= value >> shift;
    0x9E35_A7BDu32.wrapping_mul(value) >> shift
}

fn byte_mask_to_context_mask(byte_mask: u8) -> u64 {
    let mut bit_mask = 0u64;
    for i in 0..8 {
        bit_mask |= ((byte_mask >> i) & 1) as u64 * (0xffu64 << (i * 8));
    }
    bit_mask
}

fn escape_assembly_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn command_available(command: &str) -> bool {
    Command::new(command).arg("--version").output().is_ok()
}

fn assert_command_success(action: &str, output: &Output) -> Result<()> {
    if !output.status.success() {
        bail!(
            "{action} failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    Ok(())
}

struct CompressedMacho {
    compressed: Vec<u8>,
    uncompressed: Vec<u8>,
    segments: Vec<CompressedSegment>,
}

struct CompressedSegment {
    name: String,
    size: usize,
}

fn compress_binary_with_model(binary: &[u8], model: Box<dyn Model>) -> Result<CompressedMacho> {
    let macho = parser::parse(&binary)?;

    // Collect code and data segments in file order, skipping segments with no
    // file backing (__PAGEZERO has file_size == 0) and linker metadata (__LINKEDIT).
    let mut segments: Vec<(u64, String, &[u8])> = macho
        .load_commands
        .iter()
        .filter_map(|lc| {
            if let LoadCommand::Segment(seg) = lc {
                Some(seg)
            } else {
                None
            }
        })
        .filter(|seg| seg.file_size > 0 && seg.name != "__LINKEDIT")
        .map(|seg| {
            let start = seg.file_offset as usize;
            let end = start + seg.file_size as usize;
            let data = &binary[start..end];
            (seg.file_offset, seg.name.clone(), data)
        })
        .collect();

    // Sort by file offset so we compress in on-disk order.
    segments.sort_by_key(|(file_offset, _, _)| *file_offset);

    if segments.is_empty() {
        anyhow::bail!("No compressible segments found in the binary");
    }

    let mut compressed: Vec<u8> = Vec::new();
    let mut uncompressed: Vec<u8> = Vec::new();
    let segment_summaries = segments
        .iter()
        .map(|(_, name, data)| CompressedSegment {
            name: name.clone(),
            size: data.len(),
        })
        .collect();
    let mut encoder = Encoder::new(model, &mut compressed)?;

    for (_, _, data) in &segments {
        encoder.encode_section(*data)?;
        uncompressed.extend_from_slice(data);
    }
    encoder.finish()?;

    Ok(CompressedMacho {
        compressed,
        uncompressed,
        segments: segment_summaries,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        fs,
        path::{Path, PathBuf},
        process::{Command, Output},
        rc::Rc,
    };

    use crate::compressor::model::{LnMixerPred, NOrderByte};

    use super::*;

    struct HalfModel;

    impl Model for HalfModel {
        fn pred(&mut self) -> f64 {
            0. // It's squashed (1.0 / (1.0 + exp(-0.0))) = 0.5, so this model always predicts 50% probability
        }

        fn learn(&mut self, _bit: u8) {}
    }

    #[test]
    fn bootstrap_round_trips_compressed_macho_segments() {
        if !cfg!(all(target_os = "macos", target_arch = "aarch64")) || !command_available("clang") {
            return;
        }

        let binary = fs::read("tests/macho/helloworld").expect("Failed to read Mach-O fixture");
        let compressed_macho = compress_binary_with_model(&binary, Box::new(HalfModel))
            .expect("Failed to compress Mach-O fixture");

        let test_dir = PathBuf::from("./testout/macho_roundtrip");
        fs::create_dir_all(&test_dir).expect("Failed to create temp test directory");

        let compressed_path = test_dir.join("compressed.bin");
        let expected_path = test_dir.join("expected.bin");
        let payload_path = test_dir.join("payload.s");
        let harness_path = test_dir.join("harness.c");
        let output_path = test_dir.join("roundtrip");

        fs::write(&compressed_path, &compressed_macho.compressed)
            .expect("Failed to write compressed fixture");
        fs::write(&expected_path, &compressed_macho.uncompressed)
            .expect("Failed to write expected fixture");
        fs::write(
            &payload_path,
            render_payload_assembly(
                &compressed_path,
                &expected_path,
                compressed_macho.uncompressed.len(),
            ),
        )
        .expect("Failed to write payload assembly");
        fs::write(&harness_path, HARNESS_C).expect("Failed to write harness C");

        let link_output = Command::new("clang")
            .arg("-arch")
            .arg("arm64")
            .arg("src/macho/template/bootstrap.s")
            .arg("src/macho/template/decoder.s")
            .arg(&payload_path)
            .arg(&harness_path)
            .arg("-o")
            .arg(&output_path)
            .output()
            .expect("Failed to run clang");
        assert_success("link round-trip Mach-O", &link_output);

        let run_output = Command::new(&output_path)
            .output()
            .expect("Failed to run round-trip Mach-O");
        assert_success("run round-trip Mach-O", &run_output);
    }

    #[test]
    fn assembly_model_templates_round_trip() {
        if !cfg!(all(target_os = "macos", target_arch = "aarch64")) || !command_available("clang") {
            return;
        }

        let binary = fs::read("tests/macho/helloworld").expect("Failed to read Mach-O fixture");
        let table_pow2 = 10;

        run_assembly_model_round_trip(
            "norder_order0",
            &binary,
            Box::new(NOrderByte::new_norder_model(
                0,
                Rc::new(RefCell::new(HashTable::<NOrderByteData>::new(table_pow2))),
                255,
            )),
            render_norder_model_assembly(0, table_pow2, false),
        );

        run_assembly_model_round_trip(
            "norder_mask01",
            &binary,
            Box::new(NOrderByte::new_norder_model(
                0b0000_0011,
                Rc::new(RefCell::new(HashTable::<NOrderByteData>::new(table_pow2))),
                255,
            )),
            render_norder_model_assembly(0b0000_0011, table_pow2, false),
        );

        run_assembly_model_round_trip(
            "word",
            &binary,
            Box::new(NOrderByte::new_word_model(
                Rc::new(RefCell::new(HashTable::<NOrderByteData>::new(table_pow2))),
                255,
            )),
            render_norder_model_assembly(0, table_pow2, true),
        );

        let shared_table = Rc::new(RefCell::new(HashTable::<NOrderByteData>::new(table_pow2)));
        run_assembly_model_round_trip(
            "ln_mixer",
            &binary,
            Box::new(LnMixerPred::new(vec![
                Box::new(NOrderByte::new_norder_model(0, shared_table.clone(), 255)),
                Box::new(NOrderByte::new_norder_model(0b0000_0011, shared_table, 255)),
            ])),
            render_ln_mixer_model_assembly(table_pow2),
        );
    }

    #[test]
    fn full_decompressor_round_trip() {
        if !cfg!(all(target_os = "macos", target_arch = "aarch64")) || !command_available("clang") {
            return;
        }

        let binary_path = PathBuf::from("tests/macho/helloworld");
        let output_dir = PathBuf::from("./testout/macho_full_decompressor");
        run(Args {
            input: binary_path.clone(),
            output_directory: output_dir.to_string_lossy().into_owned(),
        })
        .expect("Failed to build full Mach-O decompressor");

        let decoded_path = output_dir.join("decoded_segments.bin");
        let run_output = Command::new(output_dir.join("decompressor"))
            .arg(&decoded_path)
            .output()
            .expect("Failed to run generated decompressor");
        assert_success("run generated decompressor", &run_output);

        let binary = fs::read(binary_path).expect("Failed to read Mach-O fixture");
        let model_config = create_default_model_config();
        let model = model_config
            .create_model(Rc::new(RefCell::new(HashTable::<NOrderByteData>::new(
                DEFAULT_NORDER_TABLE_POW2,
            ))))
            .expect("Failed to create default model");
        let expected = compress_binary_with_model(&binary, model)
            .expect("Failed to collect expected decoded bytes")
            .uncompressed;
        let decoded = fs::read(decoded_path).expect("Failed to read decoded output");

        assert_eq!(decoded, expected);
    }

    fn run_assembly_model_round_trip(
        name: &str,
        binary: &[u8],
        model: Box<dyn Model>,
        model_assembly: String,
    ) {
        let compressed_macho =
            compress_binary_with_model(binary, model).expect("Failed to compress Mach-O fixture");

        let test_dir = PathBuf::from(format!("./testout/macho_assembly_models/{name}"));
        fs::create_dir_all(&test_dir).expect("Failed to create temp test directory");

        let compressed_path = test_dir.join("compressed.bin");
        let expected_path = test_dir.join("expected.bin");
        let payload_path = test_dir.join("payload.s");
        let model_path = test_dir.join("model.s");
        let harness_path = test_dir.join("harness.c");
        let output_path = test_dir.join("roundtrip");

        fs::write(&compressed_path, &compressed_macho.compressed)
            .expect("Failed to write compressed fixture");
        fs::write(&expected_path, &compressed_macho.uncompressed)
            .expect("Failed to write expected fixture");
        fs::write(
            &payload_path,
            render_payload_assembly(
                &compressed_path,
                &expected_path,
                compressed_macho.uncompressed.len(),
            ),
        )
        .expect("Failed to write payload assembly");
        fs::write(&model_path, model_assembly).expect("Failed to write model assembly");
        fs::write(&harness_path, HARNESS_AFTER_DECODE_C).expect("Failed to write harness C");

        let link_output = Command::new("clang")
            .arg("-arch")
            .arg("arm64")
            .arg("src/macho/template/bootstrap.s")
            .arg("src/macho/template/decoder.s")
            .arg("src/macho/template/model_support.s")
            .arg("src/macho/template/norder_byte.s")
            .arg("src/macho/template/word.s")
            .arg("src/macho/template/ln_mixer.s")
            .arg(&payload_path)
            .arg(&model_path)
            .arg(&harness_path)
            .arg("-o")
            .arg(&output_path)
            .output()
            .expect("Failed to run clang");
        assert_success(&format!("link {name} assembly model"), &link_output);

        let run_output = Command::new(&output_path)
            .output()
            .expect("Failed to run round-trip Mach-O");
        assert_success(&format!("run {name} assembly model"), &run_output);
    }

    fn command_available(command: &str) -> bool {
        Command::new(command).arg("--version").output().is_ok()
    }

    fn render_payload_assembly(
        compressed_path: &Path,
        expected_path: &Path,
        output_len: usize,
    ) -> String {
        format!(
            r#".section __DATA,__const
.p2align 3
.globl _websqz_compressed_start
_websqz_compressed_start:
.incbin "{compressed_path}"
.globl _websqz_compressed_end
_websqz_compressed_end:

.p2align 3
.globl _websqz_expected_start
_websqz_expected_start:
.incbin "{expected_path}"
.globl _websqz_expected_end
_websqz_expected_end:

.section __DATA,__data
.p2align 3
.globl _websqz_output_start
_websqz_output_start:
.space {output_len}
.globl _websqz_output_end
_websqz_output_end:
"#,
            compressed_path = escape_assembly_path(compressed_path),
            expected_path = escape_assembly_path(expected_path),
        )
    }

    fn escape_assembly_path(path: &Path) -> String {
        path.to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    }

    fn render_norder_model_assembly(byte_mask: u8, table_pow2: u32, is_word: bool) -> String {
        let table_len = 1usize << table_pow2;
        let (magic_num, prev_bytes, mask, predict_fn, learn_fn, is_word_flag) = if is_word {
            (
                model_hash(1337, 2),
                2166136261u64,
                u64::MAX,
                "_websqz_word_predict",
                "_websqz_word_learn",
                1u32,
            )
        } else {
            (
                model_hash(byte_mask as u32, 2),
                0u64,
                byte_mask_to_context_mask(byte_mask),
                "_websqz_norder_byte_predict",
                "_websqz_norder_byte_learn",
                0u32,
            )
        };

        format!(
            r#".section __DATA,__data
.p2align 3
.globl _websqz_model_ctx
_websqz_model_ctx:
    .long 0
    .long 1
    .long 0x{magic_num:08x}
    .long 15
    .quad 0x{prev_bytes:016x}
    .quad 0x{mask:016x}
    .long {is_word_flag}
    .long 0
    .quad _websqz_norder_table
    .quad {hash_mask}

.p2align 2
_websqz_norder_table:
.space {table_bytes}

.text
.align 2
.globl _websqz_model_predict
_websqz_model_predict:
    stp     x29, x30, [sp, #-16]!
    mov     x29, sp
    bl      {predict_fn}
    bl      _websqz_prob_squash
    ldp     x29, x30, [sp], #16
    ret

.globl _websqz_model_learn
_websqz_model_learn:
    b       {learn_fn}
"#,
            hash_mask = table_len - 1,
            table_bytes = table_len * NORDER_RECORD_BYTES,
        )
    }

    fn render_ln_mixer_model_assembly(table_pow2: u32) -> String {
        let table_len = 1usize << table_pow2;
        let child0_magic = model_hash(0, 2);
        let child1_mask = 0b0000_0011;
        let child1_magic = model_hash(child1_mask, 2);
        let child1_context_mask = byte_mask_to_context_mask(child1_mask as u8);
        let ctx_rows = 256usize * 255usize;
        let num_models = 2usize;
        let ctx_weight_bytes = ctx_rows * num_models * size_of::<f64>();

        format!(
            r#".section __DATA,__data
.p2align 3
.globl _websqz_model_ctx
_websqz_model_ctx:
    .long {num_models}
    .long 1
    .long 0
    .long 0
    .double 0.0
    .quad _websqz_mixer_child_contexts
    .quad _websqz_mixer_predict_fns
    .quad _websqz_mixer_learn_fns
    .quad _websqz_mixer_base_weights
    .quad _websqz_mixer_ctx_weights
    .quad _websqz_mixer_ctx_init
    .quad _websqz_mixer_last_p

.p2align 3
_websqz_mixer_child0:
    .long 0
    .long 1
    .long 0x{child0_magic:08x}
    .long 15
    .quad 0
    .quad 0
    .long 0
    .long 0
    .quad _websqz_mixer_table
    .quad {hash_mask}

.p2align 3
_websqz_mixer_child1:
    .long 0
    .long 1
    .long 0x{child1_magic:08x}
    .long 15
    .quad 0
    .quad 0x{child1_context_mask:016x}
    .long 0
    .long 0
    .quad _websqz_mixer_table
    .quad {hash_mask}

.p2align 3
_websqz_mixer_child_contexts:
    .quad _websqz_mixer_child0
    .quad _websqz_mixer_child1
_websqz_mixer_predict_fns:
    .quad _websqz_norder_byte_predict
    .quad _websqz_norder_byte_predict
_websqz_mixer_learn_fns:
    .quad _websqz_norder_byte_learn
    .quad _websqz_norder_byte_learn
_websqz_mixer_base_weights:
    .double 0.5
    .double 0.5
_websqz_mixer_last_p:
    .space 16

.p2align 2
_websqz_mixer_table:
.space {table_bytes}

.p2align 3
_websqz_mixer_ctx_init:
    .space {ctx_rows}
.p2align 3
_websqz_mixer_ctx_weights:
    .space {ctx_weight_bytes}

.text
.align 2
.globl _websqz_model_predict
_websqz_model_predict:
    b       _websqz_ln_mixer_predict

.globl _websqz_model_learn
_websqz_model_learn:
    b       _websqz_ln_mixer_learn
"#,
            hash_mask = table_len - 1,
            table_bytes = table_len * NORDER_RECORD_BYTES,
        )
    }

    fn model_hash(mut value: u32, shift: u32) -> u32 {
        value ^= value >> shift;
        0x9E35_A7BDu32.wrapping_mul(value) >> shift
    }

    fn byte_mask_to_context_mask(byte_mask: u8) -> u64 {
        let mut bit_mask = 0u64;
        for i in 0..8 {
            bit_mask |= ((byte_mask >> i) & 1) as u64 * (0xffu64 << (i * 8));
        }
        bit_mask
    }

    fn assert_success(action: &str, output: &Output) {
        assert!(
            output.status.success(),
            "{action} failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    const HARNESS_C: &str = r#"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

extern const uint8_t websqz_expected_start[];
extern const uint8_t websqz_expected_end[];

uintptr_t websqz_model_ctx;

double websqz_model_predict(void *ctx) {
    (void)ctx;
    return 0.5;
}

void websqz_model_learn(void *ctx, uint32_t bit) {
    (void)ctx;
    (void)bit;
}

void websqz_after_decode(uint8_t *output, uint64_t len) {
    const uint8_t *expected = websqz_expected_start;
    size_t expected_len = (size_t)(websqz_expected_end - websqz_expected_start);

    if (len != expected_len) {
        fprintf(stderr, "decoded length mismatch: got %llu, expected %zu\n",
                (unsigned long long)len, expected_len);
        exit(1);
    }

    if (memcmp(output, expected, expected_len) != 0) {
        for (size_t i = 0; i < expected_len; i++) {
            if (output[i] != expected[i]) {
                fprintf(stderr,
                        "decoded byte mismatch at %zu: got 0x%02x, expected 0x%02x\n",
                        i, output[i], expected[i]);
                break;
            }
        }
        exit(1);
    }
}
"#;

    const HARNESS_AFTER_DECODE_C: &str = r#"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

extern const uint8_t websqz_expected_start[];
extern const uint8_t websqz_expected_end[];

void websqz_after_decode(uint8_t *output, uint64_t len) {
    const uint8_t *expected = websqz_expected_start;
    size_t expected_len = (size_t)(websqz_expected_end - websqz_expected_start);

    if (len != expected_len) {
        fprintf(stderr, "decoded length mismatch: got %llu, expected %zu\n",
                (unsigned long long)len, expected_len);
        exit(1);
    }

    if (memcmp(output, expected, expected_len) != 0) {
        for (size_t i = 0; i < expected_len; i++) {
            if (output[i] != expected[i]) {
                fprintf(stderr,
                        "decoded byte mismatch at %zu: got 0x%02x, expected 0x%02x\n",
                        i, output[i], expected[i]);
                break;
            }
        }
        exit(1);
    }
}
"#;
}
