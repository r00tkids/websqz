use std::{
    fs,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::compress_config::ModelConfig;
use anyhow::{anyhow, Context, Result};
use bitflags::bitflags;
use bytes::BufMut;
use handlebars::Handlebars;
use serde_json::json;

pub struct OutputGenerationOptions {
    pub output_dir: PathBuf,
    pub target: Target,
    pub model_config: ModelConfig,
}

bitflags! {
    /// Represents a set of flags.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct ModelRef: u32 {
        const None = 0b00000000;
        const NOrderByte = 0b00000001;
        const Mixer = 0b00000010;
        const AdaptiveProbabilityMap = 0b00000100;
        const Word = 0b00001000;
        const HashTable = 0b00010000;
    }
}

pub fn generate_js_decompression_code(
    model_config: &ModelConfig,
    features_used: &mut ModelRef,
) -> String {
    let mut static_src: String = "".to_owned();
    let mut out_src = "let model = ".to_owned();
    out_src += generate_js_ctors(model_config, features_used).as_str();

    out_src += ";\n";

    static_src += include_str!("js_source/hash_map.js");
    static_src += include_str!("js_source/coder.js");
    static_src += include_str!("js_source/utils.js");

    if *features_used & (ModelRef::NOrderByte | ModelRef::Word) != ModelRef::None {
        static_src += include_str!("js_source/norder_byte.js");
    }

    if features_used.contains(ModelRef::Mixer) {
        static_src += include_str!("js_source/mixer.js");
    }

    if features_used.contains(ModelRef::AdaptiveProbabilityMap) {
        static_src += include_str!("js_source/adaptive_probability_map.js");
    }

    static_src + "\n" + out_src.as_str()
}

fn generate_js_ctors(model_config: &ModelConfig, features_used: &mut ModelRef) -> String {
    match model_config {
        ModelConfig::NOrderByte { byte_mask } => {
            *features_used |= ModelRef::NOrderByte;
            *features_used |= ModelRef::HashTable;
            format!("NOrderByte({}, 0)", byte_mask)
        }
        ModelConfig::Mixer { models } => {
            *features_used |= ModelRef::Mixer;
            let models_js: Vec<String> = models
                .into_iter()
                .map(|c| generate_js_ctors(c, features_used))
                .collect();
            format!("LnMixerPred([{}])", models_js.join(", "))
        }
        ModelConfig::AdaptiveProbabilityMap(model_config) => {
            *features_used |= ModelRef::AdaptiveProbabilityMap;
            let inner_js = generate_js_ctors(model_config, features_used);
            format!("AdaptiveProbabilityMap(19, {})", inner_js)
        }
        ModelConfig::Word => {
            *features_used |= ModelRef::Word;
            "NOrderByte(0, 1)".to_string()
        }
    }
}

pub enum Target {
    Web,
    Node,
}
pub fn render_output(
    output_options: OutputGenerationOptions,
    size_before_encoding: usize,
    mut encoded_data: Vec<u8>,
) -> Result<()> {
    let OutputGenerationOptions {
        output_dir,
        target,
        model_config,
    } = output_options;

    fs::create_dir_all(&output_dir).context("Failed to create output directory")?;

    let encoded_data_path = output_dir.join("input.pack");

    let mut features_used = ModelRef::None;
    let decompression_code = generate_js_decompression_code(&model_config, &mut features_used);

    Ok(match target {
        Target::Web => {
            let html_path = output_dir.join("index.html");
            let output_file =
                fs::File::create(&html_path).expect("Failed to create index.html file");

            let decompressor_code = Handlebars::new()
                .render_template(
                    include_str!("templates/web/boot.js"),
                    &json!({
                        "decompressor_source": decompression_code
                    }),
                )
                .context("Failed to render decompression code template")?;

            let decompressor_code_ugly =
                uglify_src(&decompressor_code).expect("Failed to uglify decompression code");

            let deflated_code = deflate_text(&decompressor_code_ugly)
                .context("Failed to deflate decompression code")?;

            let mut compressed_data = Vec::<u8>::new();
            encode_compressed_data(&mut compressed_data, size_before_encoding, &encoded_data)
                .context("Failed to encode compressed data")?;

            let html_header_str = Handlebars::new()
                .render_template(
                    include_str!("templates/web/index.html"),
                    &json!({
                        "decompressor_end": 171/*161*/ + deflated_code.len(),
                    }),
                )
                .context("Failed to render html header template")?;
            let html_header_bytes = html_header_str.as_bytes();

            let mut writer = BufWriter::new(output_file);
            // Write the HTML header
            writer
                .write_all(html_header_bytes)
                .context("Failed to write HTML header")?;
            // Write the decompression code
            writer
                .write_all(deflated_code.as_slice())
                .context("Failed to write decompression code")?;
            // Write the compressed data
            writer
                .write_all(&compressed_data)
                .context("Failed to write compressed data")?;
        }
        Target::Node => {
            let mut encoded_data_file = BufWriter::new(
                fs::File::create(&encoded_data_path).context("Failed to create input.bin file")?,
            );
            encode_compressed_data(&mut encoded_data_file, size_before_encoding, &encoded_data)
                .context("Failed to encode compressed data")?;

            let index_src_path = output_dir.join("index.mjs");
            let writer =
                fs::File::create(&index_src_path).expect("Failed to create index.html file");

            let reg = Handlebars::new();
            reg.render_template_to_write(
                include_str!("templates/node/index.mjs"),
                &json!({
                    "decompressor_source": decompression_code,
                    "input_file": "input.pack",
                    "output_file": "output.bin",
                }),
                writer,
            )
            .context("Failed to render node decompressor template")?
        }
    })
}

fn deflate_text(text: &str) -> Result<Vec<u8>> {
    let mut encoded_data = Vec::new();
    let mut writer =
        flate2::write::DeflateEncoder::new(&mut encoded_data, flate2::Compression::best());
    writer.write_all(text.as_bytes())?;
    writer.finish()?;
    Ok(encoded_data)
}

fn encode_compressed_data<T: Write>(
    writer: &mut T,
    size_before_encoding: usize,
    encoded_data: &[u8],
) -> Result<()> {
    let mut header = Vec::<u8>::new();
    header.put_u32(size_before_encoding as u32);
    writer
        .write(&header)
        .context("Failed to write header to output")?;
    writer
        .write(encoded_data)
        .context("Failed to write encoded data to output")?;

    Ok(())
}

fn uglify_src(text: &str) -> Result<String> {
    let child = Command::new("uglifyjs")
        .arg("--compress")
        .arg("--mangle")
        .arg("--toplevel")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context(
            "Failed to run uglifyjs. Run 'npm install -g uglify-js' to install it globally.",
        )?;

    child
        .stdin
        .as_ref()
        .context("Failed to get stdin for uglifyjs")?
        .write_all(text.as_bytes())
        .context("Failed to write to uglifyjs stdin")?;

    let output = child
        .wait_with_output()
        .context("Failed to read output from uglifyjs")?;

    if !output.status.success() {
        return Err(anyhow!("UglifyJS failed with status: {}", output.status));
    }

    Ok(String::from_utf8(output.stdout).context("Failed to parse uglified output")?)
}
