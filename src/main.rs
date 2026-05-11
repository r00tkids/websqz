use anyhow::Result;
use clap::{Parser, Subcommand};
use human_panic::{setup_panic, Metadata};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod compressor;
mod macho;
mod output_generator;
mod report;
mod web;

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Javascript file being evaluated after decompression
    #[arg(short, long)]
    js_main: Option<String>,

    /// Files to be included and packed into the output, with compression.
    /// Order matters, so files of similar content should be ordered together.
    #[arg(short, long, value_delimiter = ',')]
    files: Vec<String>,

    /// Files to be included and packed into the output, without compression
    #[arg(short, long, value_delimiter = ',')]
    pre_compressed_files: Vec<String>,

    /// Output directory
    #[arg(short, long)]
    output_directory: Option<String>,

    /// Target platform for the output
    #[arg(short, long, default_value = "web")]
    target: output_generator::Target,

    /// If set, reports detailed compression statistics to websqz-report.html
    #[arg(short, long)]
    report: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Pack and compress JS/web assets into a self-contained output
    Web(web::Args),
    /// Compress a Mach-O binary executable
    Macho(macho::Args),
}

fn main() -> Result<()> {
    setup_panic!(
        Metadata::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
            .support("https://github.com/r00tkids/websqz/issues")
    );

    tracing_subscriber::registry()
        .with(fmt::layer().event_format(fmt::format().compact()))
        .with(EnvFilter::from_default_env())
        .try_init()?;

    let args = Cli::parse();

    match args.command {
        Some(Commands::Web(web_args)) => web::run(web_args),
        Some(Commands::Macho(macho_args)) => macho::run(macho_args),
        None => {
            let web_args = web::Args {
                js_main: args
                    .js_main
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("--js-main is required"))?,
                output_directory: args
                    .output_directory
                    .ok_or_else(|| anyhow::anyhow!("--output-directory is required"))?,
                files: args.files,
                pre_compressed_files: args.pre_compressed_files,
                target: args.target,
                report: args.report,
            };
            web::run(web_args)
        }
    }
}

#[cfg(test)]
mod node_tests {
    use std::path::PathBuf;
    use std::process::Command;
    use std::{cell::RefCell, fs::File, io::Read, path::Path, rc::Rc};

    use crate::compressor::{
        compress_config::CompressConfig,
        model::{HashTable, NOrderByteData},
        model_finder::create_default_model_config,
        Encoder,
    };
    use crate::output_generator::{self, render_output, FileWithContent, OutputGenerationOptions};

    #[test]
    pub fn round_trip() {
        let model_config = serde_json::de::from_reader::<_, CompressConfig>(
            File::open("tests/compress.json").expect("Failed to open tests/compress.json"),
        )
        .expect("Failed to parse tests/compress.json");

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

        let mut encoded_data: Vec<u8> = Vec::new();
        let mut encoder = Encoder::new(model, &mut encoded_data).unwrap();
        encoder.encode_section(input_bytes).unwrap();
        encoder.finish().unwrap();

        render_output(
            OutputGenerationOptions {
                output_dir: Path::new("testout/round_trip").to_owned(),
                target: output_generator::Target::Node,
                model_config: model_config.model,
            },
            input_bytes.len(),
            encoded_data,
            input_bytes.len(),
            vec![],
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
            File::open("tests/compress.json").expect("Failed to open tests/compress.json"),
        )
        .expect("Failed to parse tests/compress.json");

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

        let mut encoded_data: Vec<u8> = Vec::new();
        let mut encoder = Encoder::new(model, &mut encoded_data).unwrap();
        encoder.encode_section(input_bytes).unwrap();
        encoder.finish().unwrap();

        render_output(
            OutputGenerationOptions {
                output_dir: Path::new("testout/web").to_owned(),
                target: output_generator::Target::Web,
                model_config: model_config.model,
            },
            input_bytes.len(),
            encoded_data,
            input_bytes.len(),
            vec![],
            vec![FileWithContent {
                path: PathBuf::from("Cargo.toml"),
                content: std::fs::read("Cargo.toml").expect("Failed to read Cargo.toml"),
            }],
        )
        .expect("Failed to render output");
    }

    #[test]
    pub fn round_trip_random_data() {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        let model_config = create_default_model_config();

        let hash_table = HashTable::<NOrderByteData>::new(26);
        let model = model_config
            .create_model(Rc::new(RefCell::new(hash_table)))
            .expect("Failed to create model from config");

        let mut rng = StdRng::seed_from_u64(1337);
        let mut input_bytes: Vec<u8> = vec![0u8; 1 * 1024 * 1024];
        rng.fill(&mut input_bytes[..]);

        let mut encoded_data: Vec<u8> = Vec::new();
        let mut encoder = Encoder::new(model, &mut encoded_data).unwrap();
        encoder.encode_section(&input_bytes[..]).unwrap();
        encoder.finish().unwrap();

        render_output(
            OutputGenerationOptions {
                output_dir: Path::new("testout/round_trip_rand").to_owned(),
                target: output_generator::Target::Node,
                model_config: model_config,
            },
            input_bytes.len(),
            encoded_data,
            input_bytes.len(),
            vec![],
            vec![],
        )
        .expect("Failed to render output");

        Command::new("node")
            .arg("testout/round_trip_rand/index.mjs")
            .status()
            .expect("Failed to run node decompressor");

        let output_path = Path::new("testout/round_trip_rand/output.bin");
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
