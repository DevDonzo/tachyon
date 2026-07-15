use directories::ProjectDirs;
use eframe::egui::{
    self, Color32, FontId, Key, RichText, ScrollArea, Sense, TextEdit, TextStyle, TextureHandle,
    Vec2,
    text::{LayoutJob, TextFormat},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Instant,
};
use tachyon_core::{LineNumber, SearchMode, SearchQuery};
use tachyon_ingest::{MappedFile, NewlineIndex, open_and_index};
use tachyon_render::Viewport;
use tachyon_search::{SearchBatch, SearchConfig, SearchHit, SearchStage, search_streaming};
use tachyon_trace::{TimelineSpan, TraceIndex, TrackSummary, parse_spans_json};

// ── Palette (One Dark Pro / Zed-inspired) ────────────────────────────────────

const C_BG_DEEP: Color32 = Color32::from_rgb(13, 15, 21);
const C_BG: Color32 = Color32::from_rgb(20, 23, 32);
const C_SIDEBAR: Color32 = Color32::from_rgb(16, 18, 26);
const C_SURFACE: Color32 = Color32::from_rgb(28, 32, 46);
const C_ELEVATED: Color32 = Color32::from_rgb(38, 43, 60);
const C_BORDER: Color32 = Color32::from_rgb(36, 42, 58);
const C_BORDER_VIS: Color32 = Color32::from_rgb(52, 60, 82);
const C_SEPARATOR: Color32 = Color32::from_rgb(26, 30, 43);

const C_ACCENT: Color32 = Color32::from_rgb(97, 175, 239);
const C_ACCENT_DIM: Color32 = Color32::from_rgb(28, 60, 112);
const C_GREEN: Color32 = Color32::from_rgb(152, 195, 121);
const C_RED: Color32 = Color32::from_rgb(224, 108, 117);

const C_TEXT: Color32 = Color32::from_rgb(171, 178, 191);
const C_TEXT_BRIGHT: Color32 = Color32::from_rgb(215, 222, 238);
const C_TEXT_MUTED: Color32 = Color32::from_rgb(98, 108, 132);
const C_TEXT_SUBTLE: Color32 = Color32::from_rgb(66, 74, 96);
const C_LINE_NUM: Color32 = Color32::from_rgb(62, 70, 92);
const C_LINE_NUM_HIT: Color32 = Color32::from_rgb(115, 130, 160);

const C_HIT_BG: Color32 = Color32::from_rgb(90, 58, 0);
const C_HIT_FG: Color32 = Color32::from_rgb(255, 215, 88);

const LOGO_PNG: &[u8] = include_bytes!("../../../assets/logo.png");

// Log viewer geometry
const LINE_H: f32 = 19.0;
const GUTTER_W: f32 = 66.0;
const CONTENT_PAD: f32 = 10.0;

// ── Service colour palette for trace timeline ─────────────────────────────────

const TRACE_PALETTE: &[Color32] = &[
    Color32::from_rgb(97, 175, 239),
    Color32::from_rgb(152, 195, 121),
    Color32::from_rgb(229, 192, 123),
    Color32::from_rgb(224, 108, 117),
    Color32::from_rgb(198, 120, 221),
    Color32::from_rgb(86, 182, 194),
    Color32::from_rgb(209, 154, 102),
    Color32::from_rgb(236, 88, 141),
];

fn service_color(service: &str) -> Color32 {
    let hash = service.bytes().fold(5381u64, |acc, b| {
        acc.wrapping_mul(33).wrapping_add(b as u64)
    });
    TRACE_PALETTE[(hash as usize) % TRACE_PALETTE.len()]
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run(initial_path: Option<PathBuf>, chunk_size: usize) -> eframe::Result<()> {
    let icon = load_icon();
    let mut vp = egui::ViewportBuilder::default()
        .with_title("Tachyon")
        .with_inner_size([1380.0, 880.0])
        .with_min_inner_size([900.0, 560.0]);
    if let Some(icon) = icon {
        vp = vp.with_icon(icon);
    }
    eframe::run_native(
        "Tachyon",
        eframe::NativeOptions {
            viewport: vp,
            ..Default::default()
        },
        Box::new(move |cc| Ok(Box::new(TachyonGui::new(cc, initial_path, chunk_size)))),
    )
}

// ── Application state ─────────────────────────────────────────────────────────

struct TachyonGui {
    chunk_size: usize,
    logo: Option<TextureHandle>,
    config: PersistedState,
    config_path: Option<PathBuf>,
    loaded: Option<LoadedLog>,
    trace: Option<LoadedTrace>,
    open_job: Option<Receiver<Result<LoadedLog, String>>>,
    trace_job: Option<Receiver<Result<LoadedTrace, String>>>,
    search_job: Option<SearchJob>,
    status: String,
    error: Option<String>,
    search_text: String,
    last_search_key: Option<SearchKey>,
    regex_search: bool,
    case_insensitive: bool,
    max_hits: usize,
    jump_text: String,
    active_tab: AppTab,
    command_open: bool,
    command_text: String,
    saved_session_name: String,
    /// Line-indexed hit map: O(log n) per-line lookup.
    /// Replaces the previous flat Vec that required an O(total_hits) clone +
    /// O(visible_lines × total_hits) scan every rendered frame.
    search_hits: BTreeMap<u64, Vec<SearchHit>>,
    visible_hits: usize,
    background_hits: usize,
    search_batches: usize,
}

impl TachyonGui {
    fn new(
        cc: &eframe::CreationContext<'_>,
        initial_path: Option<PathBuf>,
        chunk_size: usize,
    ) -> Self {
        setup_custom_styles(&cc.egui_ctx);
        let logo = load_logo_texture(&cc.egui_ctx);
        let (config, config_path) = load_config();

        let mut app = Self {
            chunk_size,
            logo,
            config,
            config_path,
            loaded: None,
            trace: None,
            open_job: None,
            trace_job: None,
            search_job: None,
            status: "Ready".to_owned(),
            error: None,
            search_text: String::new(),
            last_search_key: None,
            regex_search: false,
            case_insensitive: false,
            max_hits: 5_000,
            jump_text: String::new(),
            active_tab: AppTab::Logs,
            command_open: false,
            command_text: String::new(),
            saved_session_name: String::new(),
            search_hits: BTreeMap::new(),
            visible_hits: 0,
            background_hits: 0,
            search_batches: 0,
        };

        if let Some(path) = initial_path {
            app.open_log(path);
        }
        app
    }

    // ── Background jobs ───────────────────────────────────────────────────────

    fn open_log(&mut self, path: PathBuf) {
        self.cancel_search();
        self.status = format!("Opening {}…", path.display());
        self.error = None;
        let chunk_size = self.chunk_size;
        let (tx, rx) = mpsc::channel();
        self.open_job = Some(rx);
        thread::spawn(move || {
            let started = Instant::now();
            let result = open_and_index(&path, chunk_size)
                .map(|(mapped, index)| LoadedLog {
                    path,
                    mapped: Arc::new(mapped),
                    index: Arc::new(index),
                    viewport: Viewport::new(42, 220),
                    opened_ms: started.elapsed().as_millis(),
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
    }

    fn open_trace(&mut self, path: PathBuf) {
        self.status = format!("Loading trace {}…", path.display());
        self.error = None;
        let (tx, rx) = mpsc::channel();
        self.trace_job = Some(rx);
        thread::spawn(move || {
            let result = fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| parse_spans_json(&bytes).map_err(|e| e.to_string()))
                .and_then(|spans| TraceIndex::build(spans).map_err(|e| e.to_string()))
                .map(|index| LoadedTrace::new(path, index));
            let _ = tx.send(result);
        });
    }

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        // Open job
        if let Some(rx) = self.open_job.take() {
            match rx.try_recv() {
                Ok(Ok(loaded)) => {
                    let name = loaded
                        .path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("file");
                    self.status = format!(
                        "Opened {}  ·  {} lines  ·  {}ms",
                        name,
                        fmt_count(loaded.index.total_lines()),
                        loaded.opened_ms,
                    );
                    self.add_recent_file(loaded.path.clone());
                    self.jump_text = loaded.viewport.top_line.0.to_string();
                    self.loaded = Some(loaded);
                    self.search_hits.clear();
                    self.last_search_key = None;
                    self.save_config();
                    ctx.request_repaint();
                }
                Ok(Err(e)) => {
                    self.error = Some(e);
                    self.status = "Open failed".to_owned();
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.open_job = Some(rx);
                    ctx.request_repaint();
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("open worker stopped before returning a result".to_owned());
                }
            }
        }

        // Trace job
        if let Some(rx) = self.trace_job.take() {
            match rx.try_recv() {
                Ok(Ok(trace)) => {
                    self.status = format!(
                        "Loaded {} spans",
                        fmt_count(trace.index.span_count() as u64)
                    );
                    self.trace = Some(trace);
                    self.active_tab = AppTab::Trace;
                    ctx.request_repaint();
                }
                Ok(Err(e)) => {
                    self.error = Some(e);
                    self.status = "Trace load failed".to_owned();
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.trace_job = Some(rx);
                    ctx.request_repaint();
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("trace worker stopped before returning a result".to_owned());
                }
            }
        }

        // Search job — drain all pending batches this frame
        let mut finished = false;
        if let Some(job) = &self.search_job {
            loop {
                match job.receiver.try_recv() {
                    Ok(SearchEvent::Batch(batch)) => {
                        self.search_batches += 1;
                        match batch.stage {
                            SearchStage::Visible => self.visible_hits += batch.hits.len(),
                            SearchStage::Background => self.background_hits += batch.hits.len(),
                        }
                        // Insert into line-indexed BTreeMap — O(log n) per hit
                        for hit in batch.hits {
                            self.search_hits.entry(hit.line.0).or_default().push(hit);
                        }
                        ctx.request_repaint();
                    }
                    Ok(SearchEvent::Done(total)) => {
                        self.status = format!(
                            "Search complete  ·  {} hit{}",
                            fmt_count(total as u64),
                            if total == 1 { "" } else { "s" },
                        );
                        finished = true;
                        self.save_config();
                        break;
                    }
                    Ok(SearchEvent::Error(e)) => {
                        self.error = Some(e);
                        finished = true;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
        }
        if finished {
            self.search_job = None;
        }
    }

    fn maybe_start_search(&mut self) {
        let Some(loaded) = &self.loaded else { return };
        let trimmed = self.search_text.trim();
        if trimmed.is_empty() {
            self.cancel_search();
            self.search_hits.clear();
            self.last_search_key = None;
            return;
        }

        let key = SearchKey {
            query: trimmed.to_owned(),
            regex: self.regex_search,
            case_insensitive: self.case_insensitive,
            max_hits: self.max_hits,
        };
        if self.last_search_key.as_ref() == Some(&key) {
            return;
        }

        let query = if self.regex_search {
            SearchQuery::regex(trimmed.to_owned())
        } else {
            SearchQuery::substring(trimmed.to_owned(), !self.case_insensitive)
        };

        let mapped = Arc::clone(&loaded.mapped);
        let index = Arc::clone(&loaded.index);
        let visible = loaded.viewport.visible_line_range(index.total_lines());

        self.cancel_search();
        self.search_hits.clear();
        self.visible_hits = 0;
        self.background_hits = 0;
        self.search_batches = 0;

        let cancelled = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let config = SearchConfig {
            visible_lines: visible,
            chunk_lines: 8_192,
            max_hits: self.max_hits,
            batch_hit_target: 256,
        };
        let wc = Arc::clone(&cancelled);
        thread::spawn(move || {
            let result = search_streaming(mapped.bytes(), &index, &query, &config, &wc, |batch| {
                let _ = tx.send(SearchEvent::Batch(batch));
            });
            match result {
                Ok(n) => {
                    let _ = tx.send(SearchEvent::Done(n));
                }
                Err(e) => {
                    let _ = tx.send(SearchEvent::Error(e.to_string()));
                }
            }
        });

        self.last_search_key = Some(key);
        self.search_job = Some(SearchJob {
            receiver: rx,
            cancelled,
        });
        self.status = "Searching…".to_owned();
    }

    fn cancel_search(&mut self) {
        if let Some(job) = &self.search_job {
            job.cancelled.store(true, Ordering::Relaxed);
        }
        self.search_job = None;
    }

    fn add_recent_file(&mut self, path: PathBuf) {
        self.config.recent_files.retain(|r| r != &path);
        self.config.recent_files.insert(0, path);
        self.config.recent_files.truncate(12);
    }

    fn save_current_session(&mut self) {
        let Some(loaded) = &self.loaded else {
            self.error = Some("open a log before saving a session".to_owned());
            return;
        };
        let name = if self.saved_session_name.trim().is_empty() {
            loaded
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("session")
                .to_owned()
        } else {
            self.saved_session_name.trim().to_owned()
        };
        let session = SavedSession {
            name,
            log_path: loaded.path.clone(),
            trace_path: self.trace.as_ref().map(|t| t.path.clone()),
            search: self.search_text.clone(),
            regex: self.regex_search,
            case_insensitive: self.case_insensitive,
            top_line: loaded.viewport.top_line.0,
        };
        self.config
            .saved_sessions
            .retain(|s| s.name != session.name);
        self.config.saved_sessions.insert(0, session);
        self.config.saved_sessions.truncate(16);
        self.save_config();
        self.status = "Session saved".to_owned();
    }

    fn load_session(&mut self, session: SavedSession) {
        self.search_text = session.search;
        self.regex_search = session.regex;
        self.case_insensitive = session.case_insensitive;
        self.jump_text = session.top_line.to_string();
        self.open_log(session.log_path);
        if let Some(tp) = session.trace_path {
            self.open_trace(tp);
        }
    }

    fn save_config(&self) {
        let Some(path) = &self.config_path else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.config) {
            let _ = fs::write(path, json);
        }
    }

    fn apply_jump(&mut self) {
        let Some(loaded) = &mut self.loaded else {
            return;
        };
        match self.jump_text.trim().parse::<u64>() {
            Ok(line) => {
                loaded
                    .viewport
                    .jump_to_line(LineNumber(line), loaded.index.total_lines());
                self.status = format!("Jumped to line {}", loaded.viewport.top_line.0);
            }
            Err(e) => self.error = Some(format!("invalid line number: {e}")),
        }
    }

    fn execute_command(&mut self) {
        let cmd = self.command_text.trim().to_owned();
        self.command_text.clear();
        self.command_open = false;

        if cmd.eq_ignore_ascii_case("open") {
            self.pick_log_file();
        } else if let Some(p) = cmd.strip_prefix("open ") {
            self.open_log(PathBuf::from(p.trim()));
        } else if let Some(q) = cmd.strip_prefix("search ") {
            self.search_text = q.trim().to_owned();
            self.last_search_key = None;
            self.maybe_start_search();
        } else if let Some(l) = cmd.strip_prefix("jump ") {
            self.jump_text = l.trim().to_owned();
            self.apply_jump();
        } else if let Some(p) = cmd.strip_prefix("trace ") {
            self.open_trace(PathBuf::from(p.trim()));
        } else if cmd.eq_ignore_ascii_case("save session") {
            self.save_current_session();
        } else if !cmd.is_empty() {
            self.error = Some(format!("unknown command: {cmd}"));
        }
    }

    fn pick_log_file(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .set_title("Open log file")
            .pick_file()
        {
            self.open_log(p);
        }
    }

    fn pick_trace_file(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .set_title("Open trace JSON / JSONL")
            .pick_file()
        {
            self.open_trace(p);
        }
    }

    fn total_hit_count(&self) -> usize {
        self.search_hits.values().map(|v| v.len()).sum()
    }
}
