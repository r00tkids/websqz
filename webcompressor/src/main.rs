use std::{fs::File, io::Read};

use compress_config::{CompressConfig, ModelConfig};
use model_finder::ModelFinder;

mod bwt;
mod coder;
mod compress_config;
mod compressor;
mod model;
mod model_finder;
mod stats;
mod utils;

fn main() {
    let model_config = serde_json::de::from_reader::<_, CompressConfig>(
        File::open("compress.json").expect("Failed to open compress.json"),
    )
    .expect("Failed to parse compress.json");

    let model = model_config
        .model
        .create_model()
        .expect("Failed to create model from config");

    let mut test_data = String::new();
    File::open("tests/ray_tracer/index.js")
        .unwrap()
        .read_to_string(&mut test_data)
        .unwrap();

    let test_bytes = test_data.as_bytes();
    stats::StatsGenerator::gather_and_dump(test_bytes, model).unwrap();
}
