use std::{cell::RefCell, fs, path::PathBuf, rc::Rc};

use anyhow::{Context, Result};

mod model;
mod parser;

use model::LoadCommand;

use crate::compressor::{
    model::{HashTable, NOrderByteData},
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

    let macho = parser::parse(&binary)?;

    // Collect code and data segments in file order, skipping segments with no
    // file backing (__PAGEZERO has file_size == 0) and linker metadata (__LINKEDIT).
    let mut segments: Vec<(String, &[u8])> = macho
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
            (seg.name.clone(), data)
        })
        .collect();

    // Sort by file offset so we compress in on-disk order.
    segments.sort_by_key(|(_, data)| data.as_ptr() as usize);

    if segments.is_empty() {
        anyhow::bail!("No compressible segments found in the binary");
    }

    let total_uncompressed: usize = segments.iter().map(|(_, d)| d.len()).sum();
    println!(
        "Found {} segment(s) to compress ({} bytes total):",
        segments.len(),
        total_uncompressed
    );
    for (name, data) in &segments {
        println!("  {:<16} {} bytes", name, data.len());
    }

    let model_config = create_default_model_config();
    let model = model_config
        .create_model(Rc::new(RefCell::new(HashTable::<NOrderByteData>::new(26))))
        .context("Failed to create compression model")?;

    let mut compressed: Vec<u8> = Vec::new();
    let mut encoder = Encoder::new(model, &mut compressed)?;

    for (_, data) in &segments {
        encoder.encode_section(*data)?;
    }
    encoder.finish()?;

    fs::create_dir_all(&args.output_directory).with_context(|| {
        format!(
            "Failed to create output directory: {}",
            args.output_directory
        )
    })?;

    let out_path = PathBuf::from(&args.output_directory).join("compressed.bin");
    fs::write(&out_path, &compressed)
        .with_context(|| format!("Failed to write {}", out_path.display()))?;

    println!(
        "Compressed {} bytes -> {} bytes ({:.1}% of original)",
        total_uncompressed,
        compressed.len(),
        100.0 * compressed.len() as f64 / total_uncompressed as f64,
    );
    println!("Output written to {}", out_path.display());

    Ok(())
}
