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
        let query = match query {
            Ok(q) => q,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
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

// ── eframe::App ───────────────────────────────────────────────────────────────

impl eframe::App for TachyonGui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_jobs(ctx);

        // Global keyboard shortcuts
        if ctx.input(|i| i.key_pressed(Key::K) && (i.modifiers.command || i.modifiers.ctrl)) {
            self.command_open = true;
        }
        if self.command_open && ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.command_open = false;
        }

        // ── Top bar ───────────────────────────────────────────────────────
        egui::TopBottomPanel::top("top_bar")
            .exact_height(46.0)
            .frame(
                egui::Frame::NONE
                    .fill(C_BG_DEEP)
                    .inner_margin(egui::Margin {
                        left: 14,
                        right: 14,
                        top: 0,
                        bottom: 0,
                    }),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    // Logo
                    if let Some(logo) = &self.logo {
                        ui.image((logo.id(), Vec2::splat(20.0)));
                        ui.add_space(6.0);
                    }

                    // Brand
                    ui.label(
                        RichText::new("TACHYON")
                            .strong()
                            .size(12.0)
                            .color(C_TEXT_BRIGHT)
                            .extra_letter_spacing(2.2),
                    );

                    ui.add_space(16.0);

                    // Action buttons
                    if ui
                        .add(
                            egui::Button::new(RichText::new("📂  Open Log").size(11.5))
                                .fill(C_SURFACE)
                                .stroke(egui::Stroke::new(1.0, C_BORDER_VIS)),
                        )
                        .clicked()
                    {
                        self.pick_log_file();
                    }
                    if ui
                        .add(
                            egui::Button::new(RichText::new("⏱  Open Trace").size(11.5))
                                .fill(C_SURFACE)
                                .stroke(egui::Stroke::new(1.0, C_BORDER_VIS)),
                        )
                        .clicked()
                    {
                        self.pick_trace_file();
                    }

                    // Right side: tabs + ⌘K
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Command palette button
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("⌘K").size(11.0).color(C_TEXT_MUTED),
                                )
                                .frame(false),
                            )
                            .on_hover_text("Command palette  (⌘K / Ctrl+K)")
                            .clicked()
                        {
                            self.command_open = true;
                        }

                        ui.add_space(6.0);

                        // Thin vertical rule
                        let r = ui.cursor();
                        ui.painter().line_segment(
                            [
                                egui::pos2(r.left(), r.top() + 10.0),
                                egui::pos2(r.left(), r.top() + 30.0),
                            ],
                            egui::Stroke::new(1.0, C_BORDER_VIS),
                        );
                        ui.add_space(10.0);

                        // View tabs
                        for (tab, label) in [
                            (AppTab::Bench, "Bench"),
                            (AppTab::Trace, "Trace"),
                            (AppTab::Logs, "Logs"),
                        ] {
                            let active = self.active_tab == tab;
                            let (fg, fill) = if active {
                                (C_TEXT_BRIGHT, C_SURFACE)
                            } else {
                                (C_TEXT_MUTED, Color32::TRANSPARENT)
                            };
                            if ui
                                .add(
                                    egui::Button::new(RichText::new(label).size(12.0).color(fg))
                                        .fill(fill)
                                        .stroke(egui::Stroke::new(
                                            if active { 1.0 } else { 0.0 },
                                            C_BORDER_VIS,
                                        )),
                                )
                                .clicked()
                            {
                                self.active_tab = tab;
                            }
                        }
                    });
                });
            });

        // ── Status bar ────────────────────────────────────────────────────
        egui::TopBottomPanel::bottom("status_bar")
            .exact_height(26.0)
            .frame(
                egui::Frame::NONE
                    .fill(C_BG_DEEP)
                    .inner_margin(egui::Margin {
                        left: 12,
                        right: 12,
                        top: 0,
                        bottom: 0,
                    }),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    // Status message
                    ui.label(RichText::new(&self.status).size(11.0).color(C_TEXT_MUTED));

                    // Error chip
                    if let Some(err) = self.error.clone() {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(format!("⚠  {}", truncate_str(&err, 90)))
                                .size(11.0)
                                .color(C_RED),
                        );
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(" ✕ ").size(10.0).color(C_TEXT_MUTED),
                                )
                                .frame(false),
                            )
                            .clicked()
                        {
                            self.error = None;
                        }
                    }

                    // Right: position info
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(loaded) = &self.loaded {
                            let top = loaded.viewport.top_line.0;
                            let total = loaded.index.total_lines();
                            ui.label(
                                RichText::new(format!(
                                    "Ln {}  ·  {} lines total",
                                    fmt_count(top),
                                    fmt_count(total),
                                ))
                                .size(11.0)
                                .color(C_TEXT_SUBTLE)
                                .monospace(),
                            );
                        }
                    });
                });
            });

        // ── Sidebar ───────────────────────────────────────────────────────
        egui::SidePanel::left("sessions")
            .resizable(true)
            .default_width(220.0)
            .min_width(150.0)
            .frame(
                egui::Frame::NONE
                    .fill(C_SIDEBAR)
                    .inner_margin(egui::Margin {
                        left: 10,
                        right: 6,
                        top: 12,
                        bottom: 10,
                    }),
            )
            .show(ctx, |ui| self.show_sidebar(ui));

        // ── Central panel ──────────────────────────────────────────────────
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(C_BG).inner_margin(0))
            .show(ctx, |ui| match self.active_tab {
                AppTab::Logs => self.show_logs(ui),
                AppTab::Trace => self.show_trace(ui),
                AppTab::Bench => self.show_bench(ui),
            });

        // ── Command palette overlay ────────────────────────────────────────
        if self.command_open {
            self.show_command_palette(ctx);
        }
    }
}

// ── Panel implementations ─────────────────────────────────────────────────────

impl TachyonGui {
    // ── Sidebar ───────────────────────────────────────────────────────────────

    fn show_sidebar(&mut self, ui: &mut egui::Ui) {
        // Section header helper
        let section_hdr = |text: &str| {
            RichText::new(text)
                .size(10.0)
                .strong()
                .color(C_TEXT_SUBTLE)
                .extra_letter_spacing(1.2)
        };

        // ── Sessions ──
        ui.label(section_hdr("SESSIONS"));
        ui.add_space(5.0);
        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut self.saved_session_name)
                    .hint_text("Session name…")
                    .desired_width(120.0),
            );
            if ui
                .add(egui::Button::new(RichText::new("Save").size(11.0)))
                .clicked()
            {
                self.save_current_session();
            }
        });
        ui.add_space(4.0);

        let sessions = self.config.saved_sessions.clone();
        if sessions.is_empty() {
            ui.label(
                RichText::new("No saved sessions")
                    .size(11.0)
                    .color(C_TEXT_SUBTLE),
            );
        } else {
            for session in sessions {
                let label = format!("📁  {}", session.name);
                if ui
                    .add(egui::SelectableLabel::new(
                        false,
                        RichText::new(label).size(11.5).color(C_TEXT),
                    ))
                    .clicked()
                {
                    self.load_session(session);
                }
            }
        }

        ui.add_space(14.0);

        // ── Recent files ──
        ui.label(section_hdr("RECENT FILES"));
        ui.add_space(5.0);

        let recent = self.config.recent_files.clone();
        if recent.is_empty() {
            ui.label(
                RichText::new("No recent files")
                    .size(11.0)
                    .color(C_TEXT_SUBTLE),
            );
        } else {
            for path in recent {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_else(|| path.to_str().unwrap_or("file"));
                if ui
                    .add(egui::SelectableLabel::new(
                        false,
                        RichText::new(format!("📄  {}", name))
                            .size(11.5)
                            .color(C_TEXT),
                    ))
                    .on_hover_text(path.display().to_string())
                    .clicked()
                {
                    self.open_log(path);
                }
            }
        }

        // Loading spinner
        if self.open_job.is_some() || self.trace_job.is_some() {
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading…").size(11.0).color(C_TEXT_SUBTLE));
            });
        }
    }

    // ── Log viewer ────────────────────────────────────────────────────────────

    fn show_logs(&mut self, ui: &mut egui::Ui) {
        // ── Search toolbar ────────────────────────────────────────────────
        egui::Frame::NONE
            .fill(C_BG)
            .inner_margin(egui::Margin {
                left: 12,
                right: 12,
                top: 8,
                bottom: 8,
            })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Search input
                    let resp = ui.add(
                        TextEdit::singleline(&mut self.search_text)
                            .hint_text("Search logs…")
                            .desired_width(280.0)
                            .font(TextStyle::Monospace),
                    );

                    // Regex toggle
                    let rc = if self.regex_search {
                        C_ACCENT
                    } else {
                        C_TEXT_SUBTLE
                    };
                    if ui
                        .add(
                            egui::Button::new(RichText::new(".*").size(11.5).color(rc))
                                .fill(if self.regex_search {
                                    C_ACCENT_DIM
                                } else {
                                    Color32::TRANSPARENT
                                })
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    if self.regex_search {
                                        C_ACCENT
                                    } else {
                                        C_BORDER
                                    },
                                )),
                        )
                        .on_hover_text("Toggle regex mode")
                        .clicked()
                    {
                        self.regex_search = !self.regex_search;
                        self.last_search_key = None;
                    }

                    // Case toggle
                    let cc_color = if !self.case_insensitive {
                        C_ACCENT
                    } else {
                        C_TEXT_SUBTLE
                    };
                    if ui
                        .add(
                            egui::Button::new(RichText::new("Aa").size(11.0).color(cc_color))
                                .fill(if !self.case_insensitive {
                                    C_ACCENT_DIM
                                } else {
                                    Color32::TRANSPARENT
                                })
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    if !self.case_insensitive {
                                        C_ACCENT
                                    } else {
                                        C_BORDER
                                    },
                                )),
                        )
                        .on_hover_text("Case-sensitive (highlighted = ON)")
                        .clicked()
                    {
                        self.case_insensitive = !self.case_insensitive;
                        self.last_search_key = None;
                    }

                    // Trigger search
                    if resp.changed()
                        || ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.any())
                    {
                        self.last_search_key = None;
                        self.maybe_start_search();
                    }

                    // Cancel
                    if self.search_job.is_some()
                        && ui
                            .add(
                                egui::Button::new(RichText::new("✕  Stop").size(11.0).color(C_RED))
                                    .stroke(egui::Stroke::new(1.0, C_RED)),
                            )
                            .clicked()
                    {
                        self.cancel_search();
                    }

                    // Vertical divider
                    ui.add_space(4.0);
                    let r = ui.cursor();
                    ui.painter().line_segment(
                        [
                            egui::pos2(r.left(), r.top() + 3.0),
                            egui::pos2(r.left(), r.top() + 23.0),
                        ],
                        egui::Stroke::new(1.0, C_BORDER_VIS),
                    );
                    ui.add_space(6.0);

                    // Jump to line
                    ui.add(
                        TextEdit::singleline(&mut self.jump_text)
                            .hint_text("Go to line…")
                            .desired_width(84.0)
                            .font(TextStyle::Monospace),
                    );
                    if ui
                        .add(egui::Button::new(RichText::new("Go").size(11.0)))
                        .clicked()
                    {
                        self.apply_jump();
                    }
                });

                // Hit-count row
                let total = self.total_hit_count();
                if total > 0 || self.search_job.is_some() {
                    ui.add_space(2.0);
                    let mut parts: Vec<String> = Vec::new();
                    if total > 0 {
                        parts.push(format!(
                            "{} hit{}",
                            fmt_count(total as u64),
                            if total == 1 { "" } else { "s" }
                        ));
                    }
                    if self.search_job.is_some() {
                        parts.push("searching…".to_owned());
                    } else if total > 0 {
                        parts.push(format!(
                            "{} visible  ·  {} background",
                            fmt_count(self.visible_hits as u64),
                            fmt_count(self.background_hits as u64),
                        ));
                    }
                    let color = if total > 0 { C_GREEN } else { C_TEXT_SUBTLE };
                    ui.label(RichText::new(parts.join("   ·   ")).size(11.0).color(color));
                }
            });

        // Separator line
        ui.painter().hline(
            ui.cursor().x_range(),
            ui.cursor().top(),
            egui::Stroke::new(1.0, C_SEPARATOR),
        );

        // ── No file loaded ─────────────────────────────────────────────────
        if self.loaded.is_none() {
            self.show_empty_state(ui);
            return;
        }

        // ── Collect viewport data ──────────────────────────────────────────
        // We collect into owned Strings here so that the borrow on self.loaded
        // ends before we need to borrow self.search_hits for hit lookups.
        let avail_h = ui.available_height().max(LINE_H);
        let vis_rows = ((avail_h / LINE_H) as u32 + 2).max(1);

        // Read scroll delta first (it's a global input, not tied to any widget)
        let scroll_delta = ui.input(|i| i.smooth_scroll_delta.y + i.raw_scroll_delta.y);

        struct ViewData {
            top_line: u64,
            total_lines: u64,
            // (line_number, line_start_byte, owned_text)
            lines: Vec<(u64, u64, String)>,
        }

        let viewport_result: Result<ViewData, String> = {
            let loaded = self.loaded.as_mut().unwrap();
            loaded.viewport.visible_rows = vis_rows;
            let total = loaded.index.total_lines();

            if scroll_delta.abs() > 0.5 {
                loaded
                    .viewport
                    .scroll_lines((-scroll_delta / LINE_H).round() as i64, total);
            }

            let top = loaded.viewport.top_line.0;
            let visible = loaded.viewport.visible_line_range(total);

            loaded
                .mapped
                .line_window(&loaded.index, visible)
                .map(|slices| ViewData {
                    top_line: top,
                    total_lines: total,
                    lines: slices
                        .iter()
                        .map(|s| {
                            (
                                s.line.0,
                                s.byte_range.start.0,
                                String::from_utf8_lossy(s.bytes).into_owned(),
                            )
                        })
                        .collect(),
                })
                .map_err(|e| e.to_string())
        }; // borrow on self.loaded ends here

        let vd = match viewport_result {
            Ok(vd) => vd,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };

        // Sync jump-text field with current scroll position
        self.jump_text = vd.top_line.to_string();

        // ── Render log lines ───────────────────────────────────────────────
        // self.search_hits is now freely borrowable (self.loaded borrow released).
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;

                let avail_w = ui.available_width();

                // Virtual space BEFORE visible lines (proportional scrollbar thumb)
                let pre_lines = vd.top_line.min(1_000_000);
                if pre_lines > 0 {
                    ui.add_space((pre_lines as f32 * LINE_H).min(40_000.0));
                }

                for (line_num, byte_start, text) in &vd.lines {
                    let row_hits = self
                        .search_hits
                        .get(line_num)
                        .map_or(&[][..], Vec::as_slice);
                    let is_hit = !row_hits.is_empty();

                    let (row_rect, row_resp) =
                        ui.allocate_exact_size(Vec2::new(avail_w, LINE_H), Sense::hover());

                    if !ui.is_rect_visible(row_rect) {
                        continue;
                    }

                    let painter = ui.painter();

                    // ── Gutter ──
                    let gutter =
                        egui::Rect::from_min_size(row_rect.min, Vec2::new(GUTTER_W, LINE_H));
                    painter.rect_filled(gutter, 0.0, C_BG_DEEP);

                    // Gutter / content separator
                    painter.line_segment(
                        [
                            egui::pos2(gutter.right(), row_rect.top()),
                            egui::pos2(gutter.right(), row_rect.bottom()),
                        ],
                        egui::Stroke::new(1.0, C_SEPARATOR),
                    );

                    // Line number text
                    painter.text(
                        egui::pos2(gutter.right() - 7.0, gutter.center().y),
                        egui::Align2::RIGHT_CENTER,
                        format!("{}", line_num + 1),
                        FontId::monospace(11.0),
                        if is_hit { C_LINE_NUM_HIT } else { C_LINE_NUM },
                    );

                    // ── Row background ──
                    let content_rect = egui::Rect::from_min_max(
                        egui::pos2(gutter.right() + 1.0, row_rect.top()),
                        row_rect.max,
                    );

                    if row_resp.hovered() {
                        painter.rect_filled(
                            row_rect,
                            0.0,
                            Color32::from_rgba_premultiplied(255, 255, 255, 6),
                        );
                    } else if is_hit {
                        painter.rect_filled(
                            content_rect,
                            0.0,
                            Color32::from_rgba_premultiplied(90, 58, 0, 25),
                        );
                    }

                    // ── Content text ──
                    let tx = content_rect.left() + CONTENT_PAD;
                    let ty = content_rect.center().y;

                    if row_hits.is_empty() {
                        painter.text(
                            egui::pos2(tx, ty),
                            egui::Align2::LEFT_CENTER,
                            text.as_str(),
                            FontId::monospace(12.0),
                            C_TEXT,
                        );
                    } else {
                        let job = line_layout_job(text, *byte_start, row_hits, ui);
                        let galley = ui.fonts(|f| f.layout_job(job));
                        painter.galley(egui::pos2(tx, ty - galley.size().y * 0.5), galley, C_TEXT);
                    }
                }

                // Virtual space AFTER visible lines
                let shown = vd.lines.len() as u64;
                let after = vd
                    .total_lines
                    .saturating_sub(vd.top_line + shown)
                    .min(1_000_000);
                if after > 0 {
                    ui.add_space((after as f32 * LINE_H).min(40_000.0));
                }
            });
    }

    // ── Empty state ───────────────────────────────────────────────────────────

    fn show_empty_state(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_size();
        let center = egui::pos2(avail.x * 0.5, avail.y * 0.38);
        let rect = egui::Rect::from_center_size(center, Vec2::new(420.0, 320.0));

        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
            ui.vertical_centered(|ui| {
                // Logo
                if let Some(logo) = &self.logo {
                    ui.image((logo.id(), Vec2::splat(68.0)));
                    ui.add_space(18.0);
                }

                // Headline
                ui.label(
                    RichText::new("Tachyon")
                        .size(28.0)
                        .strong()
                        .color(C_TEXT_BRIGHT),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new("High-performance log & trace explorer")
                        .size(13.0)
                        .color(C_TEXT_MUTED),
                );

                ui.add_space(30.0);

                // Primary CTA
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new("📂   Open a Log File")
                                .size(14.0)
                                .color(C_TEXT_BRIGHT),
                        )
                        .fill(C_ACCENT_DIM)
                        .stroke(egui::Stroke::new(1.0, C_ACCENT))
                        .min_size(Vec2::new(210.0, 38.0)),
                    )
                    .clicked()
                {
                    self.pick_log_file();
                }

                ui.add_space(10.0);
                ui.label(
                    RichText::new("or press  ⌘K  to open the command palette")
                        .size(11.5)
                        .color(C_TEXT_SUBTLE),
                );

                // Keyboard hints
                ui.add_space(28.0);
                egui::Frame::NONE
                    .fill(C_SURFACE)
                    .stroke(egui::Stroke::new(1.0, C_BORDER))
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        for (key, desc) in [
                            ("⌘K", "Command palette"),
                            ("Scroll", "Navigate log lines"),
                            ("Enter", "Run search"),
                        ] {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!("{:<8}", key))
                                        .monospace()
                                        .size(11.0)
                                        .color(C_ACCENT),
                                );
                                ui.label(RichText::new(desc).size(11.0).color(C_TEXT_MUTED));
                            });
                        }
                    });
            });
        });
    }

    // ── Trace view ────────────────────────────────────────────────────────────

    fn show_trace(&mut self, ui: &mut egui::Ui) {
        let Some(trace) = &mut self.trace else {
            ui.vertical_centered(|ui| {
                ui.add_space(90.0);
                ui.label(
                    RichText::new("Trace Timeline")
                        .size(22.0)
                        .strong()
                        .color(C_TEXT_BRIGHT),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Open an OTLP-like JSON or JSONL file to view spans")
                        .size(13.0)
                        .color(C_TEXT_MUTED),
                );
                ui.add_space(22.0);
                if ui
                    .add(
                        egui::Button::new(RichText::new("⏱   Open Trace File").size(13.0))
                            .fill(C_SURFACE)
                            .stroke(egui::Stroke::new(1.0, C_BORDER_VIS))
                            .min_size(Vec2::new(180.0, 34.0)),
                    )
                    .clicked()
                {
                    self.pick_trace_file();
                }
            });
            return;
        };

        // ── Toolbar ──
        egui::Frame::NONE
            .fill(C_BG)
            .inner_margin(egui::Margin {
                left: 12,
                right: 12,
                top: 8,
                bottom: 8,
            })
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} spans  ·  {} services",
                            fmt_count(trace.index.span_count() as u64),
                            fmt_count(trace.index.track_count() as u64),
                        ))
                        .size(12.0)
                        .color(C_TEXT_MUTED),
                    );
                    ui.add_space(14.0);
                    ui.add(egui::DragValue::new(&mut trace.window_start).prefix("start: "));
                    ui.add(egui::DragValue::new(&mut trace.window_end).prefix("end: "));
                    ui.add(
                        egui::DragValue::new(&mut trace.max_spans)
                            .range(1..=100_000)
                            .prefix("max: "),
                    );
                    if ui
                        .add(egui::Button::new(RichText::new("Query").size(11.0)))
                        .clicked()
                    {
                        trace.refresh_window();
                    }
                });
            });

        // ── Track legend ──
        let summaries = trace.track_summaries.clone();
        egui::Frame::NONE
            .fill(C_BG_DEEP)
            .inner_margin(egui::Margin {
                left: 12,
                right: 12,
                top: 6,
                bottom: 6,
            })
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for s in &summaries {
                        let col = service_color(&s.service);
                        let (dot_r, _) =
                            ui.allocate_exact_size(Vec2::new(8.0, 8.0), Sense::hover());
                        ui.painter().circle_filled(dot_r.center(), 4.0, col);
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(format!(
                                "{}  ({} spans, {} lanes)",
                                s.service, s.span_count, s.lanes
                            ))
                            .size(11.0)
                            .color(C_TEXT_MUTED),
                        );
                        ui.add_space(14.0);
                    }
                });
            });

        ui.painter().hline(
            ui.cursor().x_range(),
            ui.cursor().top(),
            egui::Stroke::new(1.0, C_SEPARATOR),
        );

        // ── Span rows ──
        let (bounds_start, bounds_end) = trace.index.time_bounds().unwrap_or((0, 1));
        let duration = bounds_end.saturating_sub(bounds_start).max(1) as f64;
        let spans = trace.window_spans.clone();

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;

                for span in &spans {
                    let avail_w = ui.available_width().max(400.0);
                    let row_h = 24.0f32;

                    let (row_rect, row_resp) =
                        ui.allocate_exact_size(Vec2::new(avail_w, row_h), Sense::hover());

                    if !ui.is_rect_visible(row_rect) {
                        continue;
                    }

                    let painter = ui.painter();

                    // Row hover bg
                    if row_resp.hovered() {
                        painter.rect_filled(row_rect, 0.0, C_SURFACE);
                    }

                    // Bar geometry
                    let t0 = (span.start_ns.saturating_sub(bounds_start) as f64 / duration) as f32;
                    let t1 = (span.end_ns.saturating_sub(bounds_start) as f64 / duration) as f32;
                    let x0 = row_rect.left() + t0 * row_rect.width();
                    let x1 = (row_rect.left() + t1 * row_rect.width()).max(x0 + 3.0);

                    let bar = egui::Rect::from_min_max(
                        egui::pos2(x0, row_rect.top() + 3.0),
                        egui::pos2(x1, row_rect.bottom() - 3.0),
                    );

                    let base_col = service_color(&span.service);
                    let bar_col = if row_resp.hovered() {
                        base_col.gamma_multiply(1.35)
                    } else {
                        base_col
                    };
                    painter.rect_filled(bar, egui::CornerRadius::same(3), bar_col);

                    // Span label
                    let label = format!("{} / {} / lane {}", span.service, span.name, span.lane);
                    let lx = (x0 + 4.0).max(row_rect.left() + 4.0);
                    painter.text(
                        egui::pos2(lx, bar.center().y),
                        egui::Align2::LEFT_CENTER,
                        &label,
                        FontId::monospace(10.5),
                        Color32::from_rgba_premultiplied(12, 12, 12, 220),
                    );

                    // Tooltip
                    if row_resp.hovered() {
                        let dur_us = span.end_ns.saturating_sub(span.start_ns) / 1_000;
                        row_resp.on_hover_ui(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{} / {}\nDuration: {} µs  (lane {})",
                                    span.service,
                                    span.name,
                                    fmt_count(dur_us),
                                    span.lane,
                                ))
                                .size(11.5)
                                .monospace(),
                            );
                        });
                    }
                }
            });
    }

    // ── Bench report ──────────────────────────────────────────────────────────

    fn show_bench(&mut self, ui: &mut egui::Ui) {
        egui::Frame::NONE
            .fill(C_BG)
            .inner_margin(egui::Margin::same(18))
            .show(ui, |ui| {
                ui.label(
                    RichText::new("Benchmark Report")
                        .size(19.0)
                        .strong()
                        .color(C_TEXT_BRIGHT),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Run the reproducible perf harness before publishing release notes.",
                    )
                    .size(12.0)
                    .color(C_TEXT_MUTED),
                );
                ui.add_space(10.0);

                // Command chip
                egui::Frame::NONE
                    .fill(C_SURFACE)
                    .stroke(egui::Stroke::new(1.0, C_BORDER))
                    .corner_radius(egui::CornerRadius::same(5))
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("$ ")
                                    .size(12.0)
                                    .color(C_TEXT_SUBTLE)
                                    .monospace(),
                            );
                            ui.label(
                                RichText::new("./scripts/perf_smoke.sh")
                                    .size(12.0)
                                    .color(C_GREEN)
                                    .monospace(),
                            );
                        });
                    });

                ui.add_space(18.0);
                ui.separator();
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Latest verified numbers")
                        .size(12.5)
                        .strong()
                        .color(C_TEXT_BRIGHT),
                );
                ui.add_space(8.0);

                let rows = [
                    ("newline_index / parallel_chunk_scan", "~41.5 GiB/s"),
                    ("search / substring_rare (visible-first)", "~93.5 GiB/s"),
                    ("search / regex_visible_first", "~5.2 GiB/s"),
                    ("render_frame_plan", "~42–43 µs"),
                    ("trace_window_query", "~666–684 µs"),
                ];

                egui::Frame::NONE
                    .fill(C_SURFACE)
                    .stroke(egui::Stroke::new(1.0, C_BORDER))
                    .corner_radius(egui::CornerRadius::same(5))
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        for (name, val) in &rows {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!("{:<52}", name))
                                        .size(11.5)
                                        .monospace()
                                        .color(C_TEXT_MUTED),
                                );
                                ui.label(RichText::new(*val).size(11.5).monospace().color(C_GREEN));
                            });
                        }
                    });
            });
    }

    // ── Command palette ───────────────────────────────────────────────────────

    fn show_command_palette(&mut self, ctx: &egui::Context) {
        // Dim overlay
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::new("cmd_dim"),
        ))
        .rect_filled(ctx.screen_rect(), 0.0, Color32::from_black_alpha(150));

        egui::Window::new("cmd_palette")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, Vec2::new(0.0, 110.0))
            .fixed_size(Vec2::new(580.0, 0.0))
            .frame(
                egui::Frame::NONE
                    .fill(C_SURFACE)
                    .stroke(egui::Stroke::new(1.0, C_BORDER_VIS))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(18)),
            )
            .show(ctx, |ui| {
                // Header
                ui.label(
                    RichText::new("Command Palette")
                        .size(13.5)
                        .strong()
                        .color(C_TEXT_BRIGHT),
                );
                ui.add_space(10.0);

                // Input
                let resp = ui.add(
                    TextEdit::singleline(&mut self.command_text)
                        .desired_width(f32::INFINITY)
                        .hint_text(
                            "open <path>  ·  search <query>  ·  jump <line>  ·  trace <path>  ·  save session",
                        )
                        .font(TextStyle::Monospace),
                );
                resp.request_focus();

                ui.add_space(10.0);
                ui.painter().hline(
                    ui.cursor().x_range(),
                    ui.cursor().top(),
                    egui::Stroke::new(1.0, C_BORDER),
                );
                ui.add_space(10.0);

                // Quick-access chips
                ui.horizontal_wrapped(|ui| {
                    for (label, cmd) in [
                        ("📂  Open log",      "open"),
                        ("⏱  Open trace",    "trace "),
                        ("🔍  Search",        "search "),
                        ("↓  Jump to line",   "jump "),
                        ("💾  Save session",  "save session"),
                    ] {
                        if ui
                            .add(
                                egui::Button::new(RichText::new(label).size(11.0))
                                    .fill(C_ELEVATED)
                                    .stroke(egui::Stroke::new(1.0, C_BORDER_VIS)),
                            )
                            .clicked()
                        {
                            self.command_text = cmd.to_owned();
                        }
                    }
                });

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let run = ui.add(
                        egui::Button::new(RichText::new("Run").size(12.0).color(C_TEXT_BRIGHT))
                            .fill(C_ACCENT_DIM)
                            .stroke(egui::Stroke::new(1.0, C_ACCENT))
                            .min_size(Vec2::new(72.0, 28.0)),
                    );
                    if run.clicked()
                        || (resp.lost_focus()
                            && ui.input(|i| i.key_pressed(Key::Enter)))
                    {
                        self.execute_command();
                    }

                    ui.add_space(8.0);
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("Cancel").size(11.0).color(C_TEXT_MUTED),
                            )
                            .frame(false),
                        )
                        .clicked()
                        || ui.input(|i| i.key_pressed(Key::Escape))
                    {
                        self.command_open = false;
                    }
                });
            });
    }
}

// ── Domain structs ────────────────────────────────────────────────────────────

struct LoadedLog {
    path: PathBuf,
    mapped: Arc<MappedFile>,
    index: Arc<NewlineIndex>,
    viewport: Viewport,
    opened_ms: u128,
}

struct LoadedTrace {
    path: PathBuf,
    index: TraceIndex,
    window_start: u64,
    window_end: u64,
    max_spans: usize,
    window_spans: Vec<TimelineSpan>,
    track_summaries: Vec<TrackSummary>,
}

impl LoadedTrace {
    fn new(path: PathBuf, index: TraceIndex) -> Self {
        let (window_start, window_end) = index.time_bounds().unwrap_or((0, 0));
        let track_summaries = index.track_summaries();
        let mut t = Self {
            path,
            index,
            window_start,
            window_end,
            max_spans: 500,
            window_spans: Vec::new(),
            track_summaries,
        };
        t.refresh_window();
        t
    }

    fn refresh_window(&mut self) {
        self.window_spans = self
            .index
            .query_window(self.window_start, self.window_end, self.max_spans)
            .unwrap_or_default();
    }
}

struct SearchJob {
    receiver: Receiver<SearchEvent>,
    cancelled: Arc<AtomicBool>,
}

enum SearchEvent {
    Batch(SearchBatch),
    Done(usize),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchKey {
    query: String,
    regex: bool,
    case_insensitive: bool,
    max_hits: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppTab {
    Logs,
    Trace,
    Bench,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct PersistedState {
    recent_files: Vec<PathBuf>,
    saved_sessions: Vec<SavedSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SavedSession {
    name: String,
    log_path: PathBuf,
    trace_path: Option<PathBuf>,
    search: String,
    regex: bool,
    case_insensitive: bool,
    top_line: u64,
}

// ── Free functions ────────────────────────────────────────────────────────────

fn load_config() -> (PersistedState, Option<PathBuf>) {
    let Some(dirs) = ProjectDirs::from("dev", "tachyon", "Tachyon") else {
        return (PersistedState::default(), None);
    };
    let path = dirs.config_dir().join("sessions.json");
    let config = fs::read_to_string(&path)
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    (config, Some(path))
}

fn load_logo_texture(ctx: &egui::Context) -> Option<TextureHandle> {
    let img = image::load_from_memory(LOGO_PNG).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let ci = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture("tachyon-logo", ci, Default::default()))
}

fn load_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(LOGO_PNG).ok()?.to_rgba8();
    Some(egui::IconData {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    })
}

/// Build a `LayoutJob` for a single line with highlighted search match ranges.
fn line_layout_job(text: &str, line_start: u64, hits: &[SearchHit], ui: &egui::Ui) -> LayoutJob {
    let mut job = LayoutJob::default();
    let font_id = TextStyle::Monospace.resolve(ui.style());

    let base = TextFormat {
        font_id: font_id.clone(),
        color: C_TEXT,
        ..Default::default()
    };
    let hi = TextFormat {
        font_id,
        color: C_HIT_FG,
        background: C_HIT_BG,
        ..Default::default()
    };

    let mut ranges: Vec<std::ops::Range<usize>> = hits
        .iter()
        .filter_map(|h| {
            let s = h.byte_range.start.0.saturating_sub(line_start) as usize;
            let e = h.byte_range.end.0.saturating_sub(line_start) as usize;
            (s < e && e <= text.len()).then_some(s..e)
        })
        .collect();
    ranges.sort_by_key(|r| r.start);

    let mut cursor = 0usize;
    for rng in ranges {
        if !text.is_char_boundary(rng.start) || !text.is_char_boundary(rng.end) {
            continue;
        }
        if cursor < rng.start {
            job.append(&text[cursor..rng.start], 0.0, base.clone());
        }
        job.append(&text[rng.start..rng.end], 0.0, hi.clone());
        cursor = rng.end;
    }
    if cursor < text.len() {
        job.append(&text[cursor..], 0.0, base);
    }
    job
}

fn setup_custom_styles(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();

    // Background layers
    v.panel_fill = C_BG;
    v.window_fill = C_SURFACE;

    // Noninteractive (labels, separators)
    v.widgets.noninteractive.bg_fill = C_BG;
    v.widgets.noninteractive.weak_bg_fill = C_BG_DEEP;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, C_BORDER);
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, C_TEXT_MUTED);
    v.widgets.noninteractive.corner_radius = egui::CornerRadius::same(4);

    // Inactive (buttons/inputs at rest)
    v.widgets.inactive.bg_fill = C_SURFACE;
    v.widgets.inactive.weak_bg_fill = C_BG;
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, C_BORDER_VIS);
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, C_TEXT_MUTED);
    v.widgets.inactive.corner_radius = egui::CornerRadius::same(4);

    // Hovered
    v.widgets.hovered.bg_fill = C_ELEVATED;
    v.widgets.hovered.weak_bg_fill = C_SURFACE;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, C_ACCENT);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, C_TEXT_BRIGHT);
    v.widgets.hovered.corner_radius = egui::CornerRadius::same(4);

    // Active / pressed
    v.widgets.active.bg_fill = C_ACCENT;
    v.widgets.active.weak_bg_fill = C_ACCENT;
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0, C_TEXT_BRIGHT);
    v.widgets.active.fg_stroke = egui::Stroke::new(2.0, C_TEXT_BRIGHT);
    v.widgets.active.corner_radius = egui::CornerRadius::same(4);

    // Open (combo boxes etc.)
    v.widgets.open.bg_fill = C_SURFACE;
    v.widgets.open.bg_stroke = egui::Stroke::new(1.0, C_ACCENT);

    // Selection
    v.selection.bg_fill = Color32::from_rgba_premultiplied(97, 175, 239, 55);
    v.selection.stroke = egui::Stroke::new(1.0, C_ACCENT);

    // Window border
    v.window_stroke = egui::Stroke::new(1.0, C_BORDER_VIS);
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 10],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(130),
    };

    v.hyperlink_color = C_ACCENT;
    ctx.set_visuals(v);

    let mut s = (*ctx.style()).clone();
    s.spacing.item_spacing = Vec2::new(7.0, 5.0);
    s.spacing.button_padding = Vec2::new(10.0, 5.0);
    s.spacing.indent = 16.0;
    ctx.set_style(s);
}

// ── Utility helpers ───────────────────────────────────────────────────────────

/// Format a `u64` with comma thousands separators.
fn fmt_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

/// Truncate a string to `max` chars, appending `…` if truncated.
fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let end = s
        .char_indices()
        .nth(max.saturating_sub(1))
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("{}…", &s[..end])
}

#[allow(dead_code)]
fn search_mode_label(query: &SearchQuery) -> &'static str {
    match query.mode {
        SearchMode::Substring { .. } => "substring",
        SearchMode::Regex => "regex",
    }
}

#[allow(dead_code)]
fn _path_label(path: &Path) -> &str {
    path.file_name()
        .and_then(|n| n.to_str())
        .or_else(|| path.to_str())
        .unwrap_or("file")
}
