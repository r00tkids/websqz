use std::{fs::File, io::Read};

use model_finder::ModelFinder;

mod bwt;
mod coder;
mod compressor;
mod model;
mod model_finder;
mod stats;
mod utils;

fn main() {
    let mut test_data = String::new();
    File::open("tests/ray_tracer/index.js")
        .unwrap()
        .read_to_string(&mut test_data)
        .unwrap();

    let test_bytes = test_data.as_bytes();
    let mut model_finder = ModelFinder::new();
    model_finder.learn_from(test_bytes).unwrap();
    let model_defs = &model_finder.model_defs;

    stats::StatsGenerator::gather_and_dump(test_bytes, model_defs).unwrap();
}
