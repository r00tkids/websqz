use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{bail, Context, Result};

use crate::compressor::compress_config::ModelConfig;

use super::{
    assembly::render_model_assembly, pack::CompressedMacho, payload::render_payload_assembly,
    DEFAULT_NORDER_TABLE_POW2,
};

const BOOTSTRAP_S: &str = include_str!("template/bootstrap.s");
const DECODER_S: &str = include_str!("template/decoder.s");
const MODEL_SUPPORT_S: &str = include_str!("template/model_support.s");
const NORDER_BYTE_S: &str = include_str!("template/norder_byte.s");
const WORD_S: &str = include_str!("template/word.s");
const LN_MIXER_S: &str = include_str!("template/ln_mixer.s");
const TINY_RUNTIME_C: &str = include_str!("template/tiny_runtime.c");
const DIAGNOSTIC_RUNTIME_C: &str = include_str!("template/diagnostic_runtime.c");

pub(super) fn build_decompressor(
    output_dir: &Path,
    model_config: &ModelConfig,
    compressed_path: &Path,
    packed: &CompressedMacho,
    diagnostics: bool,
) -> Result<PathBuf> {
    if !command_available("clang") {
        bail!("clang is required to build the Mach-O decompressor");
    }

    let build_dir = output_dir.join("build");
    fs::create_dir_all(&build_dir)
        .with_context(|| format!("Failed to create {}", build_dir.display()))?;

    let mut sources = vec![
        ("bootstrap.s", BOOTSTRAP_S.to_owned()),
        ("decoder.s", DECODER_S.to_owned()),
        ("model_support.s", MODEL_SUPPORT_S.to_owned()),
        (
            "payload.s",
            render_payload_assembly(compressed_path, packed),
        ),
        (
            "model.s",
            render_model_assembly(model_config, DEFAULT_NORDER_TABLE_POW2)?,
        ),
        ("runtime.c", render_runtime_c(diagnostics)),
    ];

    let model_features = ModelFeatures::from_config(model_config);
    if model_features.norder_byte {
        sources.push(("norder_byte.s", NORDER_BYTE_S.to_owned()));
    }
    if model_features.word {
        sources.push(("word.s", WORD_S.to_owned()));
    }
    if model_features.ln_mixer {
        sources.push(("ln_mixer.s", LN_MIXER_S.to_owned()));
    }

    let mut source_paths = Vec::with_capacity(sources.len());
    for (name, src) in sources {
        let path = build_dir.join(name);
        fs::write(&path, src).with_context(|| format!("Failed to write {}", path.display()))?;
        source_paths.push(path);
    }

    let decompressor_path = output_dir.join("decompressor");
    let mut command = Command::new("clang");
    command.arg("-arch").arg("arm64");
    command.arg("-Oz");
    command.arg("-fno-unwind-tables");
    command.arg("-fno-asynchronous-unwind-tables");
    command.arg("-Wl,-dead_strip");
    command.arg("-Wl,-x");
    command.arg("-Wl,-no_data_const");
    for path in &source_paths {
        command.arg(path);
    }
    command.arg("-o").arg(&decompressor_path);

    let output = command.output().context("Failed to run clang")?;
    assert_command_success("build Mach-O decompressor", &output)?;
    strip_decompressor(&decompressor_path);

    Ok(decompressor_path)
}

#[derive(Default)]
struct ModelFeatures {
    norder_byte: bool,
    word: bool,
    ln_mixer: bool,
}

impl ModelFeatures {
    fn from_config(config: &ModelConfig) -> Self {
        let mut features = Self::default();
        features.visit(config);
        features
    }

    fn visit(&mut self, config: &ModelConfig) {
        match config {
            ModelConfig::NOrderByte { .. } => {
                self.norder_byte = true;
            }
            ModelConfig::Mixer { models } => {
                self.ln_mixer = true;
                for model in models {
                    self.visit(model);
                }
            }
            ModelConfig::AdaptiveProbabilityMap(_) => {}
            ModelConfig::Word => {
                self.norder_byte = true;
                self.word = true;
            }
        }
    }
}

fn render_runtime_c(diagnostics: bool) -> String {
    if diagnostics {
        render_diagnostic_runtime_c()
    } else {
        render_tiny_runtime_c()
    }
}

fn render_tiny_runtime_c() -> String {
    TINY_RUNTIME_C.to_owned()
}

fn render_diagnostic_runtime_c() -> String {
    DIAGNOSTIC_RUNTIME_C.to_owned()
}

fn strip_decompressor(path: &Path) {
    let _ = Command::new("strip").arg(path).output();
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
