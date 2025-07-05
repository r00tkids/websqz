use std::{
    fs,
    io::{BufWriter, Write},
    path::Path,
};

use crate::compress_config::ModelConfig;
use anyhow::{Context, Result};
use bitflags::bitflags;
use bytes::BufMut;
use handlebars::Handlebars;
use serde_json::json;

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
    out_src += &match model_config {
        ModelConfig::NOrderByte { byte_mask } => {
            *features_used |= ModelRef::NOrderByte;
            *features_used |= ModelRef::HashTable;
            format!("NOrderByte({})", byte_mask)
        }
        ModelConfig::Mixer(model_configs) => {
            *features_used |= ModelRef::Mixer;
            let models_js: Vec<String> = model_configs
                .into_iter()
                .map(|c| generate_js_decompression_code(c, features_used))
                .collect();
            format!("LnMixerPred([{}])", models_js.join(", "))
        }
        ModelConfig::AdaptiveProbabilityMap(model_config) => {
            *features_used |= ModelRef::AdaptiveProbabilityMap;
            let inner_js = generate_js_decompression_code(model_config, features_used);
            format!("AdaptiveProbabilityMap(19, {})", inner_js)
        }
        ModelConfig::Word => {
            *features_used |= ModelRef::Word;
            "WordPred(21, 255)".to_string()
        }
    };

    out_src += ";\n";

    static_src += include_str!("js_source/hash_map.js");
    static_src += include_str!("js_source/coder.js");
    static_src += include_str!("js_source/utils.js");

    if features_used.contains(ModelRef::NOrderByte) {
        static_src += include_str!("js_source/norder_byte.js");
    }

    if features_used.contains(ModelRef::Mixer) {
        static_src += include_str!("js_source/mixer.js");
    }

    if features_used.contains(ModelRef::AdaptiveProbabilityMap) {
        static_src += include_str!("js_source/adaptive_probability_map.js");
    }

    if features_used.contains(ModelRef::Word) {
        static_src += include_str!("js_source/word.js");
    }

    static_src + "\n" + out_src.as_str()
}

pub enum Target {
    Web,
    Node,
}
pub fn render_output(
    output_dir: &Path,
    target: Target,
    model_config: &ModelConfig,
    size_before_encoding: usize,
    mut encoded_data: Vec<u8>,
) -> Result<()> {
    fs::create_dir_all(output_dir).context("Failed to create output directory")?;

    encoded_data.put_u32(size_before_encoding as u32);
    let encoded_data_path = output_dir.join("input.pack");

    let mut encoded_data_file = BufWriter::new(
        fs::File::create(&encoded_data_path).context("Failed to create input.bin file")?,
    );
    let mut header = Vec::<u8>::new();
    header.put_u32(size_before_encoding as u32);
    encoded_data_file
        .write(&header)
        .expect("Failed to write encoded data to input.bin");
    encoded_data_file.write(&encoded_data)?;

    let mut features_used = ModelRef::None;
    let decompression_code = generate_js_decompression_code(model_config, &mut features_used);

    Ok(match target {
        Target::Web => {
            let html_path = output_dir.join("index.html");
            let writer = fs::File::create(&html_path).expect("Failed to create index.html file");
            let reg = Handlebars::new();
            reg.render_template_to_write(
                include_str!("templates/web/index.html"),
                &json!({}),
                writer,
            )
            .context("Failed to render web decompressor template")?
        }
        Target::Node => {
            let html_path = output_dir.join("index.mjs");
            let writer = fs::File::create(&html_path).expect("Failed to create index.html file");

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
