use clap::Parser;
use std::path::PathBuf;
use tachyon_core::{LineNumber, Result};
use tachyon_ingest::{DEFAULT_CHUNK_SIZE, open_and_index};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "tachyon-app")]
#[command(about = "Tachyon local-first log explorer bootstrap")]
struct Args {
    /// Path to a log file.
    path: PathBuf,
    /// Chunk size used while scanning for newlines.
    #[arg(long, default_value_t = DEFAULT_CHUNK_SIZE)]
    chunk_size: usize,
    /// Print the first N logical lines as a sanity check.
    #[arg(long, default_value_t = 0)]
    sample_lines: u64,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .compact()
        .init();

    if let Err(error) = run(Args::parse()) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<()> {
    let (mapped, index) = open_and_index(&args.path, args.chunk_size)?;

    println!("file: {}", mapped.path().display());
    println!("bytes: {}", mapped.len());
    println!("newlines: {}", index.newline_count());
    println!("lines: {}", index.total_lines());

    if args.sample_lines > 0 {
        let limit = args.sample_lines.min(index.total_lines());
        for line in 0..limit {
            let range = index.line_byte_range(LineNumber(line))?;
            let text = String::from_utf8_lossy(
                &mapped.bytes()[range.start.0 as usize..range.end.0 as usize],
            );
            println!("{line:>8}: {text}");
        }
    }

    Ok(())
}
