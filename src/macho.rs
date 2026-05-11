use std::path::PathBuf;

use anyhow::{bail, Result};

mod model;
mod parser;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Input Mach-O binary executable to compress
    #[arg(short, long)]
    pub input: PathBuf,

    /// Output directory
    #[arg(short, long)]
    pub output_directory: String,
}

pub fn run(_args: Args) -> Result<()> {
    bail!("Mach-O compression is not yet implemented");
}
