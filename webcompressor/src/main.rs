use std::{cell::RefCell, fs::File, io::Read, path::Path, rc::Rc};

use compress_config::CompressConfig;
use compressor::Encoder;
use model::{HashTable, NOrderByteData};
use output_generator::render_output;

mod bwt;
mod coder;
mod compress_config;
mod compressor;
mod model;
mod model_finder;
mod output_generator;
mod stats;
mod utils;

fn main() {
    let model_config = serde_json::de::from_reader::<_, CompressConfig>(
        File::open("compress.json").expect("Failed to open compress.json"),
    )
    .expect("Failed to parse compress.json");

    let model = model_config
        .model
        .create_model(Rc::new(RefCell::new(HashTable::<NOrderByteData>::new(10))))
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
        Path::new("out"),
        output_generator::Target::Node,
        &model_config.model,
        input_bytes.len(),
        encoded_data,
    )
    .expect("Failed to render output");

    // TODO: Uncomment the following lines to enable stats gathering
    // stats::StatsGenerator::gather_and_dump(test_bytes, model).unwrap();
}

#[cfg(test)]
mod node_tests {
    use std::process::Command;
    use std::{cell::RefCell, fs::File, io::Read, os::unix::process, path::Path, rc::Rc};

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

        let model = model_config
            .model
            .create_model(Rc::new(RefCell::new(HashTable::<NOrderByteData>::new(10))))
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
            Path::new("testout/round_trip"),
            output_generator::Target::Node,
            &model_config.model,
            input_bytes.len(),
            encoded_data,
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
}
