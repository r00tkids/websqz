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

use model::{FileType, Flags, LoadCommand, SegmentCommand};

use crate::compressor::{
    compress_config::ModelConfig,
    model::{HashTable, Model, NOrderByteData},
    model_finder::create_default_model_config,
    Encoder,
};

const DEFAULT_NORDER_TABLE_POW2: u32 = 26;
const NORDER_RECORD_BYTES: usize = 4;
const MIXER_CONTEXT_ROWS: usize = 256 * 255;
const DYLD_CHAINED_IMPORT: u32 = 1;
const DYLD_CHAINED_PTR_64_OFFSET: u16 = 6;
const DYLD_CHAINED_PTR_START_NONE: u16 = 0xffff;
const DYLD_CHAINED_PTR_START_MULTI: u16 = 0x8000;
const PAGE_SIZE: u64 = 0x4000;
const FIXUP_KIND_REBASE: u32 = 0;
const FIXUP_KIND_BIND: u32 = 1;

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
    let total_uncompressed = compressed_macho.uncompressed_len;

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

    let decompressor_path =
        build_decompressor(&output_dir, &model_config, &out_path, &compressed_macho)
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
    packed: &CompressedMacho,
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
            render_payload_assembly(compressed_path, packed),
        ),
        (
            "model.s",
            render_model_assembly(model_config, DEFAULT_NORDER_TABLE_POW2)?,
        ),
        ("runtime.c", render_runtime_c()),
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

fn render_payload_assembly(compressed_path: &Path, packed: &CompressedMacho) -> String {
    let mut src = format!(
        r#".section __DATA,__const
.p2align 3
.globl _websqz_compressed_start
_websqz_compressed_start:
.incbin "{compressed_path}"
.globl _websqz_compressed_end
_websqz_compressed_end:

.p2align 3
.globl _websqz_image_size
_websqz_image_size:
    .quad {image_size}
.globl _websqz_entry_offset
_websqz_entry_offset:
    .quad {entry_offset}

.p2align 3
.globl _websqz_decode_chunks_start
_websqz_decode_chunks_start:
"#,
        compressed_path = escape_assembly_path(compressed_path),
        image_size = packed.image_size,
        entry_offset = packed.entry_offset,
    );

    for chunk in &packed.decode_chunks {
        src.push_str(&format!(
            "    .quad {offset}\n    .quad {size}\n",
            offset = chunk.offset,
            size = chunk.size,
        ));
    }
    src.push_str(
        r#".globl _websqz_decode_chunks_end
_websqz_decode_chunks_end:

.p2align 3
.globl _websqz_segments_start
_websqz_segments_start:
"#,
    );
    for segment in &packed.segments {
        src.push_str(&format!(
            "    .quad {offset}\n    .quad {size}\n    .long {init_prot}\n    .long 0\n",
            offset = segment.offset,
            size = segment.vm_size,
            init_prot = segment.init_prot,
        ));
    }
    src.push_str(
        r#".globl _websqz_segments_end
_websqz_segments_end:

.p2align 3
.globl _websqz_imports_start
_websqz_imports_start:
"#,
    );
    for (i, import) in packed.imports.iter().enumerate() {
        src.push_str(&format!(
            "    .quad L_websqz_import_{i}\n    .long {weak}\n    .long 0\n",
            weak = if import.weak { 1 } else { 0 },
        ));
    }
    src.push_str(
        r#".globl _websqz_imports_end
_websqz_imports_end:
"#,
    );
    for (i, import) in packed.imports.iter().enumerate() {
        src.push_str(&format!(
            "L_websqz_import_{i}:\n    .asciz \"{}\"\n",
            escape_assembly_string(&import.name),
        ));
    }

    src.push_str(
        r#"
.p2align 3
.globl _websqz_fixups_start
_websqz_fixups_start:
"#,
    );
    for fixup in &packed.fixups {
        src.push_str(&format!(
            "    .quad {offset}\n    .quad {target}\n    .quad {addend}\n    .long {import_index}\n    .long {high8}\n    .long {kind}\n    .long 0\n",
            offset = fixup.offset,
            target = fixup.target,
            addend = fixup.addend,
            import_index = fixup.import_index,
            high8 = fixup.high8,
            kind = fixup.kind,
        ));
    }
    src.push_str(
        r#".globl _websqz_fixups_end
_websqz_fixups_end:
"#,
    );
    src
}

fn render_runtime_c() -> String {
    r#"#include <dlfcn.h>
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

struct WebsqzSegment {
    uint64_t offset;
    uint64_t size;
    uint32_t prot;
    uint32_t reserved;
};

struct WebsqzImport {
    const char *name;
    uint32_t weak;
    uint32_t reserved;
};

struct WebsqzFixup {
    uint64_t offset;
    uint64_t target;
    uint64_t addend;
    uint32_t import_index;
    uint32_t high8;
    uint32_t kind;
    uint32_t reserved;
};

extern const uint64_t websqz_image_size;
extern const uint64_t websqz_entry_offset;
extern const struct WebsqzSegment websqz_segments_start[];
extern const struct WebsqzSegment websqz_segments_end[];
extern const struct WebsqzImport websqz_imports_start[];
extern const struct WebsqzImport websqz_imports_end[];
extern const struct WebsqzFixup websqz_fixups_start[];
extern const struct WebsqzFixup websqz_fixups_end[];

static uint64_t page_floor(uint64_t value, uint64_t page_size) {
    return value & ~(page_size - 1);
}

static uint64_t page_ceil(uint64_t value, uint64_t page_size) {
    return (value + page_size - 1) & ~(page_size - 1);
}

void *websqz_prepare_image(void) {
    void *image = mmap(NULL, (size_t)websqz_image_size, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANON, -1, 0);
    if (image == MAP_FAILED) {
        fprintf(stderr, "websqz: mmap failed: %s\n", strerror(errno));
        exit(1);
    }
    return image;
}

static uintptr_t resolve_import(uint32_t index) {
    size_t count = (size_t)(websqz_imports_end - websqz_imports_start);
    if (index >= count) {
        fprintf(stderr, "websqz: invalid import index %u\n", index);
        exit(1);
    }

    const struct WebsqzImport *import = &websqz_imports_start[index];
    const char *name = import->name;
    if (name[0] == '_') {
        name++;
    }

    void *symbol = dlsym(RTLD_DEFAULT, name);
    if (!symbol && !import->weak) {
        fprintf(stderr, "websqz: dlsym(%s) failed: %s\n", name, dlerror());
        exit(1);
    }
    return (uintptr_t)symbol;
}

static void apply_fixups(uint8_t *image) {
    for (const struct WebsqzFixup *fixup = websqz_fixups_start;
         fixup < websqz_fixups_end;
         fixup++) {
        uintptr_t *slot = (uintptr_t *)(image + fixup->offset);
        if (fixup->kind == 1) {
            *slot = resolve_import(fixup->import_index) + (uintptr_t)fixup->addend;
        } else {
            uintptr_t pointer = (uintptr_t)image + (uintptr_t)fixup->target;
            pointer |= (uintptr_t)fixup->high8 << 56;
            *slot = pointer;
        }
    }
}

static void protect_segments(uint8_t *image) {
    uint64_t page_size = (uint64_t)getpagesize();
    for (const struct WebsqzSegment *segment = websqz_segments_start;
         segment < websqz_segments_end;
         segment++) {
        if (segment->size == 0) {
            continue;
        }

        uint64_t start = page_floor(segment->offset, page_size);
        uint64_t end = page_ceil(segment->offset + segment->size, page_size);
        if (mprotect(image + start, (size_t)(end - start), (int)segment->prot) != 0) {
            fprintf(stderr, "websqz: mprotect failed: %s\n", strerror(errno));
            exit(1);
        }
    }
}

int websqz_launch_image(uint8_t *image, int argc, char **argv, char **envp) {
    apply_fixups(image);
    __builtin___clear_cache((char *)image, (char *)image + websqz_image_size);
    protect_segments(image);

    int (*entry)(int, char **, char **) =
        (int (*)(int, char **, char **))(void *)(image + websqz_entry_offset);
    return entry(argc, argv, envp);
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

fn escape_assembly_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\0', "")
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

#[derive(Debug)]
struct CompressedMacho {
    compressed: Vec<u8>,
    uncompressed: Vec<u8>,
    uncompressed_len: usize,
    image_size: u64,
    entry_offset: u64,
    decode_chunks: Vec<DecodeChunk>,
    segments: Vec<PackedSegment>,
    imports: Vec<PackedImport>,
    fixups: Vec<PackedFixup>,
}

#[derive(Debug)]
struct PackedSegment {
    name: String,
    size: usize,
    offset: u64,
    vm_size: u64,
    init_prot: u32,
}

#[derive(Debug)]
struct DecodeChunk {
    offset: u64,
    size: usize,
}

#[derive(Debug)]
struct PackedImport {
    name: String,
    weak: bool,
}

#[derive(Debug)]
struct PackedFixup {
    offset: u64,
    target: u64,
    addend: u64,
    import_index: u32,
    high8: u32,
    kind: u32,
}

fn compress_binary_with_model(binary: &[u8], model: Box<dyn Model>) -> Result<CompressedMacho> {
    let macho = parser::parse(&binary)?;
    validate_supported_macho(&macho)?;

    let macho_segments: Vec<&SegmentCommand> = macho
        .load_commands
        .iter()
        .filter_map(|lc| {
            if let LoadCommand::Segment(seg) = lc {
                Some(seg)
            } else {
                None
            }
        })
        .collect();

    let image_segments: Vec<&SegmentCommand> = macho_segments
        .iter()
        .copied()
        .filter(|seg| seg.vm_size > 0 && seg.name != "__PAGEZERO" && seg.name != "__LINKEDIT")
        .collect();
    if image_segments.is_empty() {
        bail!("No loadable app segments found in the binary");
    }
    if !image_segments.iter().any(|seg| seg.name == "__TEXT") {
        bail!("Unsupported Mach-O: missing __TEXT segment");
    }

    let min_vm_addr = image_segments
        .iter()
        .map(|seg| seg.vm_addr)
        .min()
        .expect("image segments checked above");
    let max_vm_addr = image_segments
        .iter()
        .map(|seg| seg.vm_addr.saturating_add(seg.vm_size))
        .max()
        .expect("image segments checked above");
    let image_size = align_up(max_vm_addr - min_vm_addr, PAGE_SIZE);
    let entry_offset = entry_offset(&macho, &macho_segments, min_vm_addr)?;

    let mut decode_sources: Vec<(u64, u64, String, &[u8])> = Vec::new();
    let mut packed_segments = Vec::new();
    for seg in &image_segments {
        let offset = seg.vm_addr - min_vm_addr;
        packed_segments.push(PackedSegment {
            name: seg.name.clone(),
            size: seg.file_size as usize,
            offset,
            vm_size: seg.vm_size,
            init_prot: seg.init_prot,
        });

        if seg.file_size == 0 {
            continue;
        }
        let start = seg.file_offset as usize;
        let end = start
            .checked_add(seg.file_size as usize)
            .context("Mach-O segment file range overflowed usize")?;
        let data = binary
            .get(start..end)
            .with_context(|| format!("Segment {} extends past end of file", seg.name))?;
        decode_sources.push((seg.file_offset, offset, seg.name.clone(), data));
    }

    decode_sources.sort_by_key(|(file_offset, _, _, _)| *file_offset);
    let decode_chunks = decode_sources
        .iter()
        .map(|(_, offset, _, data)| DecodeChunk {
            offset: *offset,
            size: data.len(),
        })
        .collect();

    if decode_sources.is_empty() {
        bail!("No compressible segments found in the binary");
    }

    let (imports, fixups) = parse_chained_fixups(binary, &macho, &macho_segments, min_vm_addr)?;

    let mut compressed: Vec<u8> = Vec::new();
    let mut uncompressed = Vec::with_capacity(
        decode_sources
            .iter()
            .map(|(_, _, _, data)| data.len())
            .sum(),
    );
    let mut uncompressed_len = 0usize;
    let mut encoder = Encoder::new(model, &mut compressed)?;

    for (_, _, _, data) in &decode_sources {
        encoder.encode_section(*data)?;
        uncompressed.extend_from_slice(data);
        uncompressed_len += data.len();
    }
    encoder.finish()?;

    Ok(CompressedMacho {
        compressed,
        uncompressed,
        uncompressed_len,
        image_size,
        entry_offset,
        decode_chunks,
        segments: packed_segments,
        imports,
        fixups,
    })
}

fn validate_supported_macho(macho: &model::MachoFile) -> Result<()> {
    if macho.header.file_type != FileType::Execute {
        bail!("Unsupported Mach-O: expected MH_EXECUTE");
    }
    if !macho.header.flags.contains(Flags::PIE) {
        bail!("Unsupported Mach-O: only PIE executables are supported");
    }
    if !macho
        .load_commands
        .iter()
        .any(|lc| matches!(lc, LoadCommand::EntryPoint(_)))
    {
        bail!("Unsupported Mach-O: missing LC_MAIN entry point");
    }
    for lc in &macho.load_commands {
        if let LoadCommand::DyldInfo(info) = lc {
            if info.rebase_size != 0
                || info.bind_size != 0
                || info.weak_bind_size != 0
                || info.lazy_bind_size != 0
            {
                bail!("Unsupported Mach-O: classic LC_DYLD_INFO fixups are not supported");
            }
        }
    }
    Ok(())
}

fn entry_offset(
    macho: &model::MachoFile,
    segments: &[&SegmentCommand],
    min_vm_addr: u64,
) -> Result<u64> {
    let entry_file_offset = macho
        .load_commands
        .iter()
        .find_map(|lc| {
            if let LoadCommand::EntryPoint(entry) = lc {
                Some(entry.entry_offset)
            } else {
                None
            }
        })
        .context("Unsupported Mach-O: missing LC_MAIN entry point")?;

    let entry_segment = segments
        .iter()
        .find(|seg| {
            entry_file_offset >= seg.file_offset
                && entry_file_offset < seg.file_offset.saturating_add(seg.file_size)
        })
        .context("Unsupported Mach-O: LC_MAIN entry point is outside file-backed segments")?;

    Ok(entry_segment.vm_addr + (entry_file_offset - entry_segment.file_offset) - min_vm_addr)
}

fn parse_chained_fixups(
    binary: &[u8],
    macho: &model::MachoFile,
    segments: &[&SegmentCommand],
    min_vm_addr: u64,
) -> Result<(Vec<PackedImport>, Vec<PackedFixup>)> {
    let Some(command) = macho.load_commands.iter().find_map(|lc| {
        if let LoadCommand::ChainedFixups(command) = lc {
            Some(command)
        } else {
            None
        }
    }) else {
        return Ok((Vec::new(), Vec::new()));
    };

    let blob_start = command.data_offset as usize;
    let blob_end = blob_start
        .checked_add(command.data_size as usize)
        .context("LC_DYLD_CHAINED_FIXUPS range overflowed usize")?;
    let blob = binary
        .get(blob_start..blob_end)
        .context("LC_DYLD_CHAINED_FIXUPS extends past end of file")?;

    let fixups_version = read_u32_at(blob, 0)?;
    let starts_offset = read_u32_at(blob, 4)? as usize;
    let imports_offset = read_u32_at(blob, 8)? as usize;
    let symbols_offset = read_u32_at(blob, 12)? as usize;
    let imports_count = read_u32_at(blob, 16)? as usize;
    let imports_format = read_u32_at(blob, 20)?;
    let symbols_format = read_u32_at(blob, 24)?;

    if fixups_version != 0 {
        bail!("Unsupported chained fixups version {fixups_version}");
    }
    if imports_format != DYLD_CHAINED_IMPORT {
        bail!("Unsupported chained imports format {imports_format}");
    }
    if symbols_format != 0 {
        bail!("Unsupported compressed chained import symbol table");
    }

    let imports = parse_chained_imports(blob, imports_offset, symbols_offset, imports_count)?;
    let fixups =
        parse_chained_starts(blob, starts_offset, segments, min_vm_addr, binary, &imports)?;

    Ok((imports, fixups))
}

fn parse_chained_imports(
    blob: &[u8],
    imports_offset: usize,
    symbols_offset: usize,
    imports_count: usize,
) -> Result<Vec<PackedImport>> {
    let mut imports = Vec::with_capacity(imports_count);
    for i in 0..imports_count {
        let raw = read_u32_at(blob, imports_offset + i * size_of::<u32>())?;
        let weak = ((raw >> 8) & 1) != 0;
        let name_offset = (raw >> 9) as usize;
        let name = read_null_terminated(blob, symbols_offset + name_offset)
            .with_context(|| format!("Invalid chained import symbol name at index {i}"))?;
        imports.push(PackedImport { name, weak });
    }
    Ok(imports)
}

fn parse_chained_starts(
    blob: &[u8],
    starts_offset: usize,
    segments: &[&SegmentCommand],
    min_vm_addr: u64,
    binary: &[u8],
    imports: &[PackedImport],
) -> Result<Vec<PackedFixup>> {
    let seg_count = read_u32_at(blob, starts_offset)? as usize;
    if seg_count > segments.len() {
        bail!(
            "Chained fixups reference {seg_count} segments, but Mach-O has only {}",
            segments.len()
        );
    }

    let mut fixups = Vec::new();
    for segment_index in 0..seg_count {
        let seg_info_offset = read_u32_at(blob, starts_offset + 4 + segment_index * 4)? as usize;
        if seg_info_offset == 0 {
            continue;
        }

        let seg = segments[segment_index];
        let starts = starts_offset + seg_info_offset;
        let _size = read_u32_at(blob, starts)?;
        let page_size = read_u16_at(blob, starts + 4)? as u64;
        let pointer_format = read_u16_at(blob, starts + 6)?;
        let segment_offset = read_u64_at(blob, starts + 8)?;
        let _max_valid_pointer = read_u32_at(blob, starts + 16)?;
        let page_count = read_u16_at(blob, starts + 20)? as usize;

        if pointer_format != DYLD_CHAINED_PTR_64_OFFSET {
            bail!(
                "Unsupported chained pointer format {pointer_format} in segment {}",
                seg.name
            );
        }
        let expected_segment_offset = seg.vm_addr - min_vm_addr;
        if segment_offset != expected_segment_offset {
            bail!(
                "Unsupported chained fixup segment offset for {}: got {segment_offset:#x}, expected {expected_segment_offset:#x}",
                seg.name
            );
        }

        for page_index in 0..page_count {
            let page_start = read_u16_at(blob, starts + 22 + page_index * 2)?;
            if page_start == DYLD_CHAINED_PTR_START_NONE {
                continue;
            }
            if (page_start & DYLD_CHAINED_PTR_START_MULTI) != 0 {
                bail!("Unsupported chained fixups with multiple starts per page");
            }

            let mut fixup_offset =
                segment_offset + page_index as u64 * page_size + page_start as u64;
            loop {
                let raw = read_u64_at(
                    binary,
                    file_offset_for_image_offset(segments, min_vm_addr, fixup_offset)?,
                )?;
                let bind = (raw >> 63) != 0;
                let next = (raw >> 51) & 0x0fff;

                if bind {
                    let import_index = (raw & 0x00ff_ffff) as u32;
                    if import_index as usize >= imports.len() {
                        bail!("Chained fixup references invalid import index {import_index}");
                    }
                    let addend = (raw >> 24) & 0xff;
                    fixups.push(PackedFixup {
                        offset: fixup_offset,
                        target: 0,
                        addend,
                        import_index,
                        high8: 0,
                        kind: FIXUP_KIND_BIND,
                    });
                } else {
                    fixups.push(PackedFixup {
                        offset: fixup_offset,
                        target: raw & 0x0000_000f_ffff_ffff,
                        addend: 0,
                        import_index: 0,
                        high8: ((raw >> 36) & 0xff) as u32,
                        kind: FIXUP_KIND_REBASE,
                    });
                }

                if next == 0 {
                    break;
                }
                fixup_offset = fixup_offset
                    .checked_add(next * 4)
                    .context("Chained fixup offset overflowed")?;
            }
        }
    }

    Ok(fixups)
}

fn file_offset_for_image_offset(
    segments: &[&SegmentCommand],
    min_vm_addr: u64,
    image_offset: u64,
) -> Result<usize> {
    for seg in segments {
        if seg.name == "__PAGEZERO" || seg.name == "__LINKEDIT" || seg.file_size == 0 {
            continue;
        }
        let start = seg.vm_addr - min_vm_addr;
        let end = start + seg.file_size;
        if image_offset >= start && image_offset + size_of::<u64>() as u64 <= end {
            return Ok((seg.file_offset + (image_offset - start)) as usize);
        }
    }
    bail!("Chained fixup at image offset {image_offset:#x} is outside file-backed segments")
}

fn read_u16_at(data: &[u8], offset: usize) -> Result<u16> {
    let bytes = data
        .get(offset..offset + 2)
        .with_context(|| format!("unexpected end of data at offset {offset}"))?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32_at(data: &[u8], offset: usize) -> Result<u32> {
    let bytes = data
        .get(offset..offset + 4)
        .with_context(|| format!("unexpected end of data at offset {offset}"))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u64_at(data: &[u8], offset: usize) -> Result<u64> {
    let bytes = data
        .get(offset..offset + 8)
        .with_context(|| format!("unexpected end of data at offset {offset}"))?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn read_null_terminated(data: &[u8], offset: usize) -> Result<String> {
    let bytes = data
        .get(offset..)
        .with_context(|| format!("unexpected end of data at offset {offset}"))?;
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .context("unterminated string")?;
    Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
}

fn align_up(value: u64, align: u64) -> u64 {
    (value + align - 1) & !(align - 1)
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

        let run_output = Command::new(output_dir.join("decompressor"))
            .output()
            .expect("Failed to run generated decompressor");
        assert_success("run generated decompressor", &run_output);
        assert_eq!(run_output.stdout, b"Hello, World!\n");
    }

    #[test]
    fn packed_macho_metadata_for_helloworld() {
        let binary = fs::read("tests/macho/helloworld").expect("Failed to read Mach-O fixture");
        let packed = compress_binary_with_model(&binary, Box::new(HalfModel))
            .expect("Failed to pack Mach-O fixture");

        assert_eq!(packed.image_size, 0x8000);
        assert_eq!(packed.entry_offset, 0x460);
        assert_eq!(packed.uncompressed_len, 0x8000);

        assert_eq!(packed.segments.len(), 2);
        assert_eq!(packed.segments[0].name, "__TEXT");
        assert_eq!(packed.segments[0].offset, 0);
        assert_eq!(packed.segments[0].vm_size, 0x4000);
        assert_eq!(packed.segments[0].init_prot, 5);
        assert_eq!(packed.segments[1].name, "__DATA_CONST");
        assert_eq!(packed.segments[1].offset, 0x4000);
        assert_eq!(packed.segments[1].vm_size, 0x4000);

        assert_eq!(packed.decode_chunks.len(), 2);
        assert_eq!(packed.decode_chunks[0].offset, 0);
        assert_eq!(packed.decode_chunks[0].size, 0x4000);
        assert_eq!(packed.decode_chunks[1].offset, 0x4000);
        assert_eq!(packed.decode_chunks[1].size, 0x4000);

        assert_eq!(packed.imports.len(), 1);
        assert_eq!(packed.imports[0].name, "_printf");
        assert!(!packed.imports[0].weak);

        assert_eq!(packed.fixups.len(), 1);
        assert_eq!(packed.fixups[0].kind, FIXUP_KIND_BIND);
        assert_eq!(packed.fixups[0].offset, 0x4000);
        assert_eq!(packed.fixups[0].import_index, 0);
    }

    #[test]
    fn rejects_missing_lc_main() {
        let mut binary = fs::read("tests/macho/helloworld").expect("Failed to read Mach-O fixture");
        patch_load_command(&mut binary, 0x8000_0028, 0x1b);

        let err = compress_binary_with_model(&binary, Box::new(HalfModel))
            .expect_err("binary without LC_MAIN should be rejected");
        assert!(format!("{err:#}").contains("missing LC_MAIN"));
    }

    #[test]
    fn rejects_unsupported_chained_pointer_format() {
        let mut binary = fs::read("tests/macho/helloworld").expect("Failed to read Mach-O fixture");
        let macho = parser::parse(&binary).expect("Failed to parse Mach-O fixture");
        let fixups = macho
            .load_commands
            .iter()
            .find_map(|lc| {
                if let LoadCommand::ChainedFixups(fixups) = lc {
                    Some(fixups)
                } else {
                    None
                }
            })
            .expect("LC_DYLD_CHAINED_FIXUPS not found");
        let blob_start = fixups.data_offset as usize;
        let starts_offset = read_u32_at(&binary, blob_start + 4).unwrap() as usize;
        let seg_info_offset =
            read_u32_at(&binary, blob_start + starts_offset + 4 + 2 * 4).unwrap() as usize;
        let pointer_format_offset = blob_start + starts_offset + seg_info_offset + 6;
        binary[pointer_format_offset..pointer_format_offset + 2]
            .copy_from_slice(&2u16.to_le_bytes());

        let err = compress_binary_with_model(&binary, Box::new(HalfModel))
            .expect_err("unsupported chained pointer format should be rejected");
        assert!(format!("{err:#}").contains("Unsupported chained pointer format"));
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

    fn patch_load_command(binary: &mut [u8], old_cmd: u32, new_cmd: u32) {
        let ncmds = u32::from_le_bytes(binary[16..20].try_into().unwrap()) as usize;
        let mut offset = 32usize;
        for _ in 0..ncmds {
            let cmd = u32::from_le_bytes(binary[offset..offset + 4].try_into().unwrap());
            let cmdsize =
                u32::from_le_bytes(binary[offset + 4..offset + 8].try_into().unwrap()) as usize;
            if cmd == old_cmd {
                binary[offset..offset + 4].copy_from_slice(&new_cmd.to_le_bytes());
                return;
            }
            offset += cmdsize;
        }
        panic!("load command {old_cmd:#x} not found");
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

.p2align 3
.globl _websqz_image_size
_websqz_image_size:
    .quad {output_len}
.globl _websqz_entry_offset
_websqz_entry_offset:
    .quad 0

.p2align 3
.globl _websqz_decode_chunks_start
_websqz_decode_chunks_start:
    .quad 0
    .quad {output_len}
.globl _websqz_decode_chunks_end
_websqz_decode_chunks_end:

.p2align 3
.globl _websqz_segments_start
_websqz_segments_start:
.globl _websqz_segments_end
_websqz_segments_end:

.p2align 3
.globl _websqz_imports_start
_websqz_imports_start:
.globl _websqz_imports_end
_websqz_imports_end:

.p2align 3
.globl _websqz_fixups_start
_websqz_fixups_start:
.globl _websqz_fixups_end
_websqz_fixups_end:
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
extern const uint64_t websqz_image_size;

uintptr_t websqz_model_ctx;

double websqz_model_predict(void *ctx) {
    (void)ctx;
    return 0.5;
}

void websqz_model_learn(void *ctx, uint32_t bit) {
    (void)ctx;
    (void)bit;
}

void *websqz_prepare_image(void) {
    uint8_t *output = calloc(1, (size_t)websqz_image_size);
    if (!output) {
        fprintf(stderr, "calloc failed\n");
        exit(1);
    }
    return output;
}

int websqz_launch_image(uint8_t *output, int argc, char **argv, char **envp) {
    (void)argc;
    (void)argv;
    (void)envp;
    const uint8_t *expected = websqz_expected_start;
    size_t expected_len = (size_t)(websqz_expected_end - websqz_expected_start);

    if ((size_t)websqz_image_size != expected_len) {
        fprintf(stderr, "decoded length mismatch: got %llu, expected %zu\n",
                (unsigned long long)websqz_image_size, expected_len);
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
    free(output);
    return 0;
}
"#;

    const HARNESS_AFTER_DECODE_C: &str = r#"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

extern const uint8_t websqz_expected_start[];
extern const uint8_t websqz_expected_end[];
extern const uint64_t websqz_image_size;

void *websqz_prepare_image(void) {
    uint8_t *output = calloc(1, (size_t)websqz_image_size);
    if (!output) {
        fprintf(stderr, "calloc failed\n");
        exit(1);
    }
    return output;
}

int websqz_launch_image(uint8_t *output, int argc, char **argv, char **envp) {
    (void)argc;
    (void)argv;
    (void)envp;
    const uint8_t *expected = websqz_expected_start;
    size_t expected_len = (size_t)(websqz_expected_end - websqz_expected_start);

    if ((size_t)websqz_image_size != expected_len) {
        fprintf(stderr, "decoded length mismatch: got %llu, expected %zu\n",
                (unsigned long long)websqz_image_size, expected_len);
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
    free(output);
    return 0;
}
"#;
}
