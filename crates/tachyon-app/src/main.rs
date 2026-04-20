mod cli;
mod gui;

use clap::Parser;
use std::path::PathBuf;
use tachyon_core::Result;
use tachyon_ingest::DEFAULT_CHUNK_SIZE;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser, Clone)]
#[command(name = "tachyon-app")]
#[command(about = "Tachyon local-first log and trace explorer")]
struct Args {
    /// Path to a log file. If omitted, Tachyon starts the desktop app.
    path: Option<PathBuf>,
    /// Start the desktop app even when a path is provided.
    #[arg(long, default_value_t = false)]
    gui: bool,
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
    /// Print per-frame render plan output.
    #[arg(long, default_value_t = false)]
    print_render_plan: bool,
    /// Number of simulated frames to plan.
    #[arg(long, default_value_t = 1)]
    render_sim_frames: u32,
    /// Maximum number of glyph uploads permitted per frame.
    #[arg(long, default_value_t = 256)]
    max_glyph_uploads: usize,
    /// Optional trace JSON/JSONL file for timeline querying.
    #[arg(long)]
    trace_json: Option<PathBuf>,
    /// Optional trace window start (ns). Defaults to trace min start.
    #[arg(long)]
    trace_window_start_ns: Option<u64>,
    /// Optional trace window end (ns). Defaults to trace max end.
    #[arg(long)]
    trace_window_end_ns: Option<u64>,
    /// Maximum number of trace spans to return.
    #[arg(long, default_value_t = 500)]
    trace_max_spans: usize,
    /// Print individual spans for the trace window.
    #[arg(long, default_value_t = false)]
    print_trace_spans: bool,
    /// Print per-service track summaries.
    #[arg(long, default_value_t = false)]
    print_trace_tracks: bool,
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
    if args.gui || args.path.is_none() {
        return gui::run(args.path, args.chunk_size)
            .map_err(|error| tachyon_core::TachyonError::Parse(error.to_string()));
    }

    cli::run(cli::CliArgs {
        path: args.path.expect("path checked above"),
        chunk_size: args.chunk_size,
        sample_lines: args.sample_lines,
        rows: args.rows,
        overscan: args.overscan,
        jump_line: args.jump_line,
        scroll_lines: args.scroll_lines,
        print_viewport: args.print_viewport,
        search: args.search,
        regex: args.regex,
        case_insensitive: args.case_insensitive,
        max_hits: args.max_hits,
        search_chunk_lines: args.search_chunk_lines,
        search_batch_hits: args.search_batch_hits,
        print_search_hits: args.print_search_hits,
        print_render_plan: args.print_render_plan,
        render_sim_frames: args.render_sim_frames,
        max_glyph_uploads: args.max_glyph_uploads,
        trace_json: args.trace_json,
        trace_window_start_ns: args.trace_window_start_ns,
        trace_window_end_ns: args.trace_window_end_ns,
        trace_max_spans: args.trace_max_spans,
        print_trace_spans: args.print_trace_spans,
        print_trace_tracks: args.print_trace_tracks,
    })
}
