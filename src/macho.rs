use std::{cell::RefCell, fs, path::PathBuf, rc::Rc};

use anyhow::{Context, Result};

mod model;
mod parser;

use model::LoadCommand;

use crate::compressor::{
    model::{HashTable, Model, NOrderByteData},
    model_finder::create_default_model_config,
    Encoder,
};

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
        .create_model(Rc::new(RefCell::new(HashTable::<NOrderByteData>::new(26))))
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

    fs::create_dir_all(&args.output_directory).with_context(|| {
        format!(
            "Failed to create output directory: {}",
            args.output_directory
        )
    })?;

    let out_path = PathBuf::from(&args.output_directory).join("compressed.bin");
    fs::write(&out_path, &compressed_macho.compressed)
        .with_context(|| format!("Failed to write {}", out_path.display()))?;

    println!(
        "Compressed {} bytes -> {} bytes ({:.1}% of original)",
        total_uncompressed,
        compressed_macho.compressed.len(),
        100.0 * compressed_macho.compressed.len() as f64 / total_uncompressed as f64,
    );
    println!("Output written to {}", out_path.display());

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
.rept {table_len}
    .long 0x007fffff
.endr

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
.rept {table_len}
    .long 0x007fffff
.endr

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
