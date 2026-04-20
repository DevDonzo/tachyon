use clap::Parser;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use tachyon_core::{LineNumber, Result, SearchQuery};
use tachyon_ingest::{DEFAULT_CHUNK_SIZE, open_and_index};
use tachyon_render::Viewport;
use tachyon_search::{SearchConfig, SearchStage, search_streaming};
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
    /// Viewport visible row count.
    #[arg(long, default_value_t = 40)]
    rows: u32,
    /// Extra rows to fetch above and below visible lines.
    #[arg(long, default_value_t = 200)]
    overscan: u32,
    /// Jump viewport top near this line.
    #[arg(long)]
    jump_line: Option<u64>,
    /// Scroll viewport by this many lines after jump.
    #[arg(long, default_value_t = 0)]
    scroll_lines: i64,
    /// Print the viewport fetch window.
    #[arg(long, default_value_t = false)]
    print_viewport: bool,
    /// Search query to run against the file.
    #[arg(long)]
    search: Option<String>,
    /// Interpret --search as regex.
    #[arg(long, default_value_t = false)]
    regex: bool,
    /// Make substring search case-insensitive.
    #[arg(long, default_value_t = false)]
    case_insensitive: bool,
    /// Maximum number of matches to return for one search.
    #[arg(long, default_value_t = 500)]
    max_hits: usize,
    /// Number of lines in each background search chunk.
    #[arg(long, default_value_t = 8192)]
    search_chunk_lines: u64,
    /// Target number of hits per streamed batch.
    #[arg(long, default_value_t = 128)]
    search_batch_hits: usize,
    /// Print individual search hits.
    #[arg(long, default_value_t = false)]
    print_search_hits: bool,
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
    let total_lines = index.total_lines();
    let mut viewport = Viewport::new(args.rows, args.overscan);

    if let Some(jump_line) = args.jump_line {
        viewport.jump_to_line(LineNumber(jump_line), total_lines);
    }
    if args.scroll_lines != 0 {
        viewport.scroll_lines(args.scroll_lines, total_lines);
    }

    let visible = viewport.visible_line_range(total_lines);
    let fetch = viewport.fetch_line_range(total_lines);

    println!("file: {}", mapped.path().display());
    println!("bytes: {}", mapped.len());
    println!("newlines: {}", index.newline_count());
    println!("lines: {total_lines}");
    println!(
        "viewport: top={} visible=[{}..{}) fetch=[{}..{})",
        viewport.top_line.0, visible.start.0, visible.end.0, fetch.start.0, fetch.end.0
    );

    if args.sample_lines > 0 {
        let limit = args.sample_lines.min(total_lines);
        for line in 0..limit {
            let line_slice = mapped.line_slice(&index, LineNumber(line))?;
            let text = String::from_utf8_lossy(line_slice.bytes);
            println!("{line:>8}: {text}");
        }
    }

    if args.print_viewport {
        let window = mapped.line_window(&index, fetch)?;
        for line_slice in window {
            let marker =
                if line_slice.line.0 >= visible.start.0 && line_slice.line.0 < visible.end.0 {
                    '>'
                } else {
                    ' '
                };
            let text = String::from_utf8_lossy(line_slice.bytes);
            println!(
                "{marker}{:>8}: [{:>10}..{:>10}) {text}",
                line_slice.line.0, line_slice.byte_range.start.0, line_slice.byte_range.end.0
            );
        }
    }

    if let Some(search_pattern) = args.search {
        let query = if args.regex {
            SearchQuery::regex(search_pattern)?
        } else {
            SearchQuery::substring(search_pattern, !args.case_insensitive)?
        };
        let config = SearchConfig {
            visible_lines: visible.clone(),
            chunk_lines: args.search_chunk_lines.max(1),
            max_hits: args.max_hits,
            batch_hit_target: args.search_batch_hits.max(1),
        };
        let cancelled = AtomicBool::new(false);
        let print_search_hits = args.print_search_hits;
        let mut visible_hits = 0usize;
        let mut background_hits = 0usize;
        let mut batches = 0usize;

        let emitted = search_streaming(
            mapped.bytes(),
            &index,
            &query,
            &config,
            &cancelled,
            |batch| {
                batches += 1;
                let stage = batch.stage;
                let hits = batch.hits;
                match stage {
                    SearchStage::Visible => visible_hits += hits.len(),
                    SearchStage::Background => background_hits += hits.len(),
                }

                if print_search_hits {
                    for hit in hits {
                        match mapped.line_slice(&index, hit.line) {
                            Ok(line_slice) => {
                                let text = String::from_utf8_lossy(line_slice.bytes);
                                println!(
                                    "search {:?} line={} match=[{}..{}) text={}",
                                    stage,
                                    hit.line.0,
                                    hit.byte_range.start.0,
                                    hit.byte_range.end.0,
                                    text
                                );
                            }
                            Err(error) => {
                                eprintln!(
                                    "search {:?} line={} could not render hit: {}",
                                    stage, hit.line.0, error
                                );
                            }
                        }
                    }
                }
            },
        )?;

        println!(
            "search: hits={} visible_hits={} background_hits={} batches={}",
            emitted, visible_hits, background_hits, batches
        );
    }

    Ok(())
}
