use std::{
    cell::RefCell,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    rc::Rc,
};

use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use compressor::Encoder;
use model::{HashTable, NOrderByteData};
use output_generator::{render_output, OutputGenerationOptions};

use crate::model_finder::create_default_model_config;

mod coder;
mod compress_config;
mod compressor;
mod model;
mod model_finder;
mod output_generator;
mod stats;
mod utils;

/// Command-line arguments
#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// Javascript file being evaluated after decompression
    #[arg(short, long, required = true)]
    js_main: String,

    /// Files to be included and packed into the output, without compression
    #[arg(short, long, value_delimiter = ',')]
    extra_pre_compressed_files: Vec<String>,

    /// Output directory
    #[arg(short, long)]
    output_directory: String,

    /// Target platform for the output
    #[arg(short, long, default_value = "web")]
    target: output_generator::Target,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.js_main.is_empty() {
        bail!("No JS main file specified");
    }

    let model_config = create_default_model_config();

    let model = model_config
        .create_model(Rc::new(RefCell::new(HashTable::<NOrderByteData>::new(26))))
        .context("Failed to create model from config")?;

    let mut input = String::new();
    File::open(&args.js_main)
        .context(format!("Failed to open JS main file: {}", args.js_main))?
        .read_to_string(&mut input)?;

    let input_bytes = input.as_bytes();

    let encoded_data: Vec<u8> = Vec::new();
    let encoder = Encoder::new(model, encoded_data)?;
    let encoded_data = encoder.encode_bytes(input_bytes)?;

    let extra_files: Result<Vec<output_generator::FileWithContent>> = args
        .extra_pre_compressed_files
        .into_iter()
        .map(|path| {
            let content =
                std::fs::read(&path).context(format!("Failed to read extra file: {}", path))?;
            return Ok(output_generator::FileWithContent {
                path: PathBuf::from(&path),
                content,
            });
        })
        .collect();

    render_output(
        OutputGenerationOptions {
            output_dir: Path::new(&args.output_directory).to_owned(),
            target: args.target,
            model_config: model_config,
        },
        input_bytes.len(),
        encoded_data,
        extra_files?,
    )
    .context("Failed to render output")?;

    Ok(())
}

#[cfg(test)]
mod node_tests {
    use std::path::PathBuf;
    use std::process::Command;
    use std::{cell::RefCell, fs::File, io::Read, path::Path, rc::Rc};

    use crate::output_generator::{FileWithContent, OutputGenerationOptions};
    use crate::{
        compress_config::CompressConfig,
        compressor::Encoder,
        model::{HashTable, NOrderByteData},
        output_generator::{self, render_output},
    };

    #[test]
    pub fn round_trip() {
        let model_config = serde_json::de::from_reader::<_, CompressConfig>(
            File::open("compress.json").expect("Failed to open compress.json"),
        )
        .expect("Failed to parse compress.json");

        let hash_table = HashTable::<NOrderByteData>::new(26);
        let model = model_config
            .model
            .create_model(Rc::new(RefCell::new(hash_table)))
            .expect("Failed to create model from config");

        let mut input = String::new();
        File::open("tests/ray_tracer/index.js")
            .unwrap()
            .read_to_string(&mut input)
            .unwrap();

        let input_bytes = input.as_bytes();

        let encoded_data: Vec<u8> = Vec::new();
        let encoder = Encoder::new(model, encoded_data).unwrap();
        let encoded_data = encoder.encode_bytes(input_bytes).unwrap();

        render_output(
            OutputGenerationOptions {
                output_dir: Path::new("testout/round_trip").to_owned(),
                target: output_generator::Target::Node,
                model_config: model_config.model,
            },
            input_bytes.len(),
            encoded_data,
            vec![],
        )
        .expect("Failed to render output");

        Command::new("node")
            .arg("testout/round_trip/index.mjs")
            .status()
            .expect("Failed to run node decompressor");

        let output_path = Path::new("testout/round_trip/output.bin");
        let output_file = File::open(output_path).expect("Failed to open output.bin");
        let mut output_data = Vec::new();
        output_file
            .take(usize::MAX as u64)
            .read_to_end(&mut output_data)
            .expect("Failed to read output.bin");

        assert_eq!(
            input_bytes,
            output_data.as_slice(),
            "Decompressed data does not match original input"
        );
    }

    #[test]
    pub fn web() {
        let model_config = serde_json::de::from_reader::<_, CompressConfig>(
            File::open("compress.json").expect("Failed to open compress.json"),
        )
        .expect("Failed to parse compress.json");

        let hash_table = HashTable::<NOrderByteData>::new(26);
        let model = model_config
            .model
            .create_model(Rc::new(RefCell::new(hash_table)))
            .expect("Failed to create model from config");

        let mut input = String::new();
        File::open(
            "tests/ray_tracer/index.js", /*"tests/reore/reore_decompressed.bin"*/
        )
        .unwrap()
        .read_to_string(&mut input)
        .unwrap();

        let input_bytes = input.as_bytes();

        let encoded_data: Vec<u8> = Vec::new();
        let encoder = Encoder::new(model, encoded_data).unwrap();
        let encoded_data = encoder.encode_bytes(input_bytes).unwrap();

        render_output(
            OutputGenerationOptions {
                output_dir: Path::new("testout/web").to_owned(),
                target: output_generator::Target::Web,
                model_config: model_config.model,
            },
            input_bytes.len(),
            encoded_data,
            vec![FileWithContent {
                path: PathBuf::from("Cargo.toml"),
                content: std::fs::read("Cargo.toml").expect("Failed to read Cargo.toml"),
            }],
        )
        .expect("Failed to render output");
    }
}
