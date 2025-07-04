use std::{
    cell::RefCell,
    fs::File,
    io::{Read, Write},
    path::Path,
    rc::Rc,
};

use compress_config::{CompressConfig, ModelConfig};
use compressor::Encoder;
use model::{HashTable, NOrderByteData};
use model_finder::ModelFinder;
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

    let mut test_data = String::new();
    File::open("tests/ray_tracer/index.js")
        .unwrap()
        .read_to_string(&mut test_data)
        .unwrap();

    let test_bytes = test_data.as_bytes();

    let encoded_data: Vec<u8> = Vec::new();
    let mut encoder = Encoder::new(model, encoded_data).unwrap();
    let encoded_data = encoder.encode_bytes(test_bytes).unwrap();

    render_output(
        Path::new("out"),
        output_generator::Target::Node,
        &model_config.model,
        test_bytes.len(),
        encoded_data,
    );

    // TODO: Uncomment the following lines to enable stats gathering
    // stats::StatsGenerator::gather_and_dump(test_bytes, model).unwrap();
}
