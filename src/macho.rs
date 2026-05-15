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
        fs,
        path::{Path, PathBuf},
        process::{Command, Output},
    };

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
}
