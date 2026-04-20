use directories::ProjectDirs;
use eframe::egui::{
    self, Color32, FontId, Key, RichText, ScrollArea, Sense, TextEdit, TextStyle, TextureHandle,
    Vec2,
    text::{LayoutJob, TextFormat},
};
use serde::{Deserialize, Serialize};
use std::{
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

const LOGO_PNG: &[u8] = include_bytes!("../../../assets/logo.png");

pub fn run(initial_path: Option<PathBuf>, chunk_size: usize) -> eframe::Result<()> {
    let icon = load_icon();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Tachyon")
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([900.0, 560.0]);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Tachyon",
        options,
        Box::new(move |cc| Ok(Box::new(TachyonGui::new(cc, initial_path, chunk_size)))),
    )
}

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
    search_hits: Vec<SearchHit>,
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
            search_hits: Vec::new(),
            visible_hits: 0,
            background_hits: 0,
            search_batches: 0,
        };

        if let Some(path) = initial_path {
            app.open_log(path);
        }

        app
    }

    fn open_log(&mut self, path: PathBuf) {
        self.cancel_search();
        self.status = format!("Opening {}...", path.display());
        self.error = None;
        let chunk_size = self.chunk_size;
        let (sender, receiver) = mpsc::channel();
        self.open_job = Some(receiver);
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
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
        });
    }

    fn open_trace(&mut self, path: PathBuf) {
        self.status = format!("Loading trace {}...", path.display());
        self.error = None;
        let (sender, receiver) = mpsc::channel();
        self.trace_job = Some(receiver);
        thread::spawn(move || {
            let result = fs::read(&path)
                .map_err(|error| error.to_string())
                .and_then(|bytes| parse_spans_json(&bytes).map_err(|error| error.to_string()))
                .and_then(|spans| TraceIndex::build(spans).map_err(|error| error.to_string()))
                .map(|index| LoadedTrace::new(path, index));
            let _ = sender.send(result);
        });
    }

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = self.open_job.take() {
            match receiver.try_recv() {
                Ok(Ok(loaded)) => {
                    self.status = format!(
                        "Opened {} lines in {} ms",
                        loaded.index.total_lines(),
                        loaded.opened_ms
                    );
                    self.add_recent_file(loaded.path.clone());
                    self.jump_text = loaded.viewport.top_line.0.to_string();
                    self.loaded = Some(loaded);
                    self.search_hits.clear();
                    self.last_search_key = None;
                    self.save_config();
                    ctx.request_repaint();
                }
                Ok(Err(error)) => {
                    self.error = Some(error);
                    self.status = "Open failed".to_owned();
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.open_job = Some(receiver);
                    ctx.request_repaint();
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("open worker stopped before returning a result".to_owned());
                }
            }
        }

        if let Some(receiver) = self.trace_job.take() {
            match receiver.try_recv() {
                Ok(Ok(trace)) => {
                    self.status = format!("Loaded {} trace spans", trace.index.span_count());
                    self.trace = Some(trace);
                    self.active_tab = AppTab::Trace;
                    ctx.request_repaint();
                }
                Ok(Err(error)) => {
                    self.error = Some(error);
                    self.status = "Trace load failed".to_owned();
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.trace_job = Some(receiver);
                    ctx.request_repaint();
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("trace worker stopped before returning a result".to_owned());
                }
            }
        }

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
                        self.search_hits.extend(batch.hits);
                    }
                    Ok(SearchEvent::Done(total)) => {
                        self.status = format!("Search complete: {total} hits");
                        finished = true;
                        self.save_config();
                        break;
                    }
                    Ok(SearchEvent::Error(error)) => {
                        self.error = Some(error);
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
        let Some(loaded) = &self.loaded else {
            return;
        };
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
            Ok(query) => query,
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };

        let mapped = Arc::clone(&loaded.mapped);
        let index = Arc::clone(&loaded.index);
        let visible_lines = loaded.viewport.visible_line_range(index.total_lines());

        self.cancel_search();
        self.search_hits.clear();
        self.visible_hits = 0;
        self.background_hits = 0;
        self.search_batches = 0;

        let cancelled = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let config = SearchConfig {
            visible_lines,
            chunk_lines: 8_192,
            max_hits: self.max_hits,
            batch_hit_target: 256,
        };
        let worker_cancelled = Arc::clone(&cancelled);
        thread::spawn(move || {
            let result = search_streaming(
                mapped.bytes(),
                &index,
                &query,
                &config,
                &worker_cancelled,
                |batch| {
                    let _ = sender.send(SearchEvent::Batch(batch));
                },
            );
            match result {
                Ok(total) => {
                    let _ = sender.send(SearchEvent::Done(total));
                }
                Err(error) => {
                    let _ = sender.send(SearchEvent::Error(error.to_string()));
                }
            }
        });

        self.last_search_key = Some(key);
        self.search_job = Some(SearchJob {
            receiver,
            cancelled,
        });
        self.status = "Searching...".to_owned();
    }

    fn cancel_search(&mut self) {
        if let Some(job) = &self.search_job {
            job.cancelled.store(true, Ordering::Relaxed);
        }
        self.search_job = None;
    }

    fn add_recent_file(&mut self, path: PathBuf) {
        self.config.recent_files.retain(|recent| recent != &path);
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
                .and_then(|name| name.to_str())
                .unwrap_or("Tachyon session")
                .to_owned()
        } else {
            self.saved_session_name.trim().to_owned()
        };

        let session = SavedSession {
            name,
            log_path: loaded.path.clone(),
            trace_path: self.trace.as_ref().map(|trace| trace.path.clone()),
            search: self.search_text.clone(),
            regex: self.regex_search,
            case_insensitive: self.case_insensitive,
            top_line: loaded.viewport.top_line.0,
        };
        self.config
            .saved_sessions
            .retain(|existing| existing.name != session.name);
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
        if let Some(trace_path) = session.trace_path {
            self.open_trace(trace_path);
        }
    }

    fn save_config(&self) {
        let Some(path) = &self.config_path else {
            return;
        };
        if let Some(parent) = path.parent()
            && fs::create_dir_all(parent).is_err()
        {
            return;
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
            Err(error) => self.error = Some(format!("invalid line number: {error}")),
        }
    }

    fn execute_command(&mut self) {
        let command = self.command_text.trim().to_owned();
        self.command_text.clear();
        self.command_open = false;

        if command.eq_ignore_ascii_case("open") {
            self.pick_log_file();
        } else if let Some(path) = command.strip_prefix("open ") {
            self.open_log(PathBuf::from(path.trim()));
        } else if let Some(query) = command.strip_prefix("search ") {
            self.search_text = query.trim().to_owned();
            self.last_search_key = None;
            self.maybe_start_search();
        } else if let Some(line) = command.strip_prefix("jump ") {
            self.jump_text = line.trim().to_owned();
            self.apply_jump();
        } else if let Some(path) = command.strip_prefix("trace ") {
            self.open_trace(PathBuf::from(path.trim()));
        } else if command.eq_ignore_ascii_case("save session") {
            self.save_current_session();
        } else if !command.is_empty() {
            self.error = Some(format!("unknown command: {command}"));
        }
    }

    fn pick_log_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Open log file")
            .pick_file()
        {
            self.open_log(path);
        }
    }

    fn pick_trace_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Open trace JSON or JSONL")
            .pick_file()
        {
            self.open_trace(path);
        }
    }
}

impl eframe::App for TachyonGui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_jobs(ctx);

        if ctx.input(|input| {
            input.key_pressed(Key::K) && (input.modifiers.command || input.modifiers.ctrl)
        }) {
            self.command_open = true;
        }

        egui::TopBottomPanel::top("top_bar")
            .inner_margin(egui::Margin::symmetric(12.0, 8.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if let Some(logo) = &self.logo {
                        ui.image((logo.id(), Vec2::splat(24.0)));
                    }
                    ui.heading(RichText::new("TACHYON").strong().letter_spacing(1.2));
                    ui.add_space(20.0);

                    ui.style_mut().spacing.button_padding = Vec2::new(8.0, 4.0);

                    if ui.add(egui::Button::new("📂 Open Log")).clicked() {
                        self.pick_log_file();
                    }
                    if ui.add(egui::Button::new("⏱ Open Trace")).clicked() {
                        self.pick_trace_file();
                    }
                    ui.add_space(10.0);
                    if ui.add(egui::Button::new("⌨ Command (⌘K)")).clicked() {
                        self.command_open = true;
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(10.0);
                        ui.selectable_value(&mut self.active_tab, AppTab::Bench, "Bench");
                        ui.selectable_value(&mut self.active_tab, AppTab::Trace, "Trace");
                        ui.selectable_value(&mut self.active_tab, AppTab::Logs, "Logs");
                    });
                });
            });

        egui::TopBottomPanel::bottom("status_bar")
            .inner_margin(egui::Margin::symmetric(10.0, 4.0))
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.visuals_mut().override_text_color = Some(Color32::from_gray(160));
                    ui.label(&self.status);
                    if let Some(error) = &self.error {
                        ui.separator();
                        ui.colored_label(Color32::from_rgb(230, 90, 90), error);
                        if ui.button("Clear").clicked() {
                            self.error = None;
                        }
                    }
                });
            });

        egui::SidePanel::left("sessions")
            .resizable(true)
            .default_width(220.0)
            .frame(
                egui::Frame::none()
                    .fill(ctx.style().visuals.window_fill())
                    .inner_margin(12.0),
            )
            .show(ctx, |ui| self.show_sidebar(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::none().inner_margin(12.0))
            .show(ctx, |ui| match self.active_tab {
                AppTab::Logs => self.show_logs(ui),
                AppTab::Trace => self.show_trace(ui),
                AppTab::Bench => self.show_bench(ui),
            });

        if self.command_open {
            self.show_command_palette(ctx);
        }
    }
}

impl TachyonGui {
    fn show_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("SESSIONS")
                .small()
                .strong()
                .color(Color32::from_gray(120)),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add(TextEdit::singleline(&mut self.saved_session_name).hint_text("Name..."));
            if ui.button("Save").clicked() {
                self.save_current_session();
            }
        });

        ui.add_space(12.0);
        ui.label(
            RichText::new("SAVED")
                .small()
                .strong()
                .color(Color32::from_gray(120)),
        );
        let sessions = self.config.saved_sessions.clone();
        for session in sessions {
            if ui
                .add(egui::SelectableLabel::new(
                    false,
                    format!("📁 {}", session.name),
                ))
                .clicked()
            {
                self.load_session(session);
            }
        }

        ui.add_space(12.0);
        ui.label(
            RichText::new("RECENT")
                .small()
                .strong()
                .color(Color32::from_gray(120)),
        );
        let recent_files = self.config.recent_files.clone();
        for path in recent_files {
            let label = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_else(|| path.to_str().unwrap_or("file"));
            if ui
                .add(egui::SelectableLabel::new(false, format!("📄 {}", label)))
                .on_hover_text(path.display().to_string())
                .clicked()
            {
                self.open_log(path);
            }
        }
    }

    fn show_logs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let changed = ui
                .add(
                    TextEdit::singleline(&mut self.search_text)
                        .desired_width(320.0)
                        .hint_text("🔍 Search logs (regex or substring)..."),
                )
                .changed();
            ui.checkbox(&mut self.regex_search, "Regex");
            ui.checkbox(&mut self.case_insensitive, "Aa");

            if changed
                || ui.button("Run").clicked()
                || ui.input(|input| input.key_pressed(Key::Enter))
            {
                self.last_search_key = None;
                self.maybe_start_search();
            }

            ui.separator();
            ui.add(
                TextEdit::singleline(&mut self.jump_text)
                    .desired_width(60.0)
                    .hint_text("Line"),
            );
            if ui.button("Jump").clicked() {
                self.apply_jump();
            }

            if self.search_job.is_some() {
                ui.add_space(8.0);
                if ui.add(egui::Button::new("🚫 Cancel")).clicked() {
                    self.cancel_search();
                }
            }
        });

        if !self.search_hits.is_empty() || self.search_job.is_some() {
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "Hits: {} | Visible: {} | Background: {}",
                    self.search_hits.len(),
                    self.visible_hits,
                    self.background_hits
                ))
                .small()
                .color(Color32::from_gray(140)),
            );
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        let Some(loaded) = &mut self.loaded else {
            self.show_empty_state(ui);
            return;
        };

        let total_lines = loaded.index.total_lines();
        let scroll_delta = ui.input(|input| input.smooth_scroll_delta.y + input.raw_scroll_delta.y);
        if scroll_delta.abs() > 0.0 {
            loaded
                .viewport
                .scroll_lines((-scroll_delta / 18.0).round() as i64, total_lines);
            self.jump_text = loaded.viewport.top_line.0.to_string();
        }

        let visible = loaded.viewport.visible_line_range(total_lines);
        let hits = self.search_hits.clone();
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                match loaded.mapped.line_window(&loaded.index, visible.clone()) {
                    Ok(lines) => {
                        for line in lines {
                            let text = String::from_utf8_lossy(line.bytes);
                            let row_hits = hits
                                .iter()
                                .filter(|hit| hit.line == line.line)
                                .collect::<Vec<_>>();
                            ui.horizontal(|ui| {
                                ui.monospace(format!("{:>10}", line.line.0));
                                ui.separator();
                                if row_hits.is_empty() {
                                    ui.add(egui::Label::new(text).sense(Sense::click()));
                                } else {
                                    ui.label(line_layout_job(
                                        &text,
                                        line.byte_range.start.0,
                                        &row_hits,
                                        ui,
                                    ));
                                }
                            });
                        }
                    }
                    Err(error) => {
                        self.error = Some(error.to_string());
                    }
                }
            });
    }

    fn show_empty_state(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.2);
            if let Some(logo) = &self.logo {
                ui.image((logo.id(), Vec2::splat(128.0)));
            }
            ui.add_space(16.0);
            ui.heading(RichText::new("Welcome to Tachyon").strong());
            ui.add_space(8.0);
            ui.label(
                RichText::new("High-performance observability workstation")
                    .color(Color32::from_gray(160)),
            );
            ui.add_space(24.0);
            if ui
                .add(egui::Button::new(
                    RichText::new("🚀 Open a Log File to Start").heading(),
                ))
                .clicked()
            {
                self.pick_log_file();
            }
            ui.add_space(12.0);
            ui.label(
                RichText::new("Or drag and drop a file here")
                    .small()
                    .color(Color32::from_gray(100)),
            );
        });
    }

    fn show_trace(&mut self, ui: &mut egui::Ui) {
        let Some(trace) = &mut self.trace else {
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                ui.heading("Trace Timeline");
                ui.label("Open an OTLP-like JSON or JSONL span file.");
                if ui.button("Open Trace File").clicked() {
                    self.pick_trace_file();
                }
            });
            return;
        };

        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{} spans across {} services",
                trace.index.span_count(),
                trace.index.track_count()
            ));
            ui.add(egui::DragValue::new(&mut trace.window_start).prefix("start "));
            ui.add(egui::DragValue::new(&mut trace.window_end).prefix("end "));
            ui.add(
                egui::DragValue::new(&mut trace.max_spans)
                    .range(1..=100_000)
                    .prefix("max "),
            );
            if ui.button("Query").clicked() {
                trace.refresh_window();
            }
        });
        ui.separator();

        for track in trace.track_summaries.clone() {
            ui.label(format!(
                "{}: {} spans, {} lanes",
                track.service, track.span_count, track.lanes
            ));
        }
        ui.separator();

        let (bounds_start, bounds_end) = trace.index.time_bounds().unwrap_or((0, 1));
        let duration = bounds_end.saturating_sub(bounds_start).max(1) as f32;
        ScrollArea::vertical().show(ui, |ui| {
            for span in &trace.window_spans {
                let available = ui.available_width().max(320.0);
                let (rect, _) = ui.allocate_exact_size(Vec2::new(available, 24.0), Sense::hover());
                let x0 = rect.left()
                    + ((span.start_ns.saturating_sub(bounds_start) as f32 / duration)
                        * rect.width());
                let x1 = rect.left()
                    + ((span.end_ns.saturating_sub(bounds_start) as f32 / duration) * rect.width());
                let bar = egui::Rect::from_min_max(
                    egui::pos2(x0, rect.top() + 4.0),
                    egui::pos2(x1.max(x0 + 2.0), rect.bottom() - 4.0),
                );
                ui.painter()
                    .rect_filled(bar, 3.0, Color32::from_rgb(56, 189, 248));
                ui.painter().text(
                    rect.left_top() + Vec2::new(4.0, 4.0),
                    egui::Align2::LEFT_TOP,
                    format!("{} / lane {} / {}", span.service, span.lane, span.name),
                    FontId::monospace(12.0),
                    Color32::WHITE,
                );
            }
        });
    }

    fn show_bench(&mut self, ui: &mut egui::Ui) {
        ui.heading("Benchmark Report");
        ui.label("Run the reproducible perf harness before publishing release notes.");
        ui.monospace("./scripts/perf_smoke.sh");
        ui.separator();
        ui.label("Latest verified local smoke numbers:");
        ui.monospace("newline_index/parallel_chunk_scan: ~41.5 GiB/s");
        ui.monospace("search/substring_rare_match_visible_first: ~93.5 GiB/s");
        ui.monospace("search/regex_visible_first: ~5.2 GiB/s");
        ui.monospace("render_frame_plan: ~42-43 us");
        ui.monospace("trace_window_query: ~666-684 us");
    }

    fn show_command_palette(&mut self, ctx: &egui::Context) {
        let mut open = self.command_open;
        egui::Window::new("Command Palette")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Commands: open, open <path>, search <query>, jump <line>, trace <path>, save session");
                let response = ui.add(
                    TextEdit::singleline(&mut self.command_text)
                        .desired_width(520.0)
                        .hint_text("type a command"),
                );
                response.request_focus();
                if ui.button("Run").clicked()
                    || (response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)))
                {
                    self.execute_command();
                }
            });
        self.command_open = open;
    }
}

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
        let mut trace = Self {
            path,
            index,
            window_start,
            window_end,
            max_spans: 500,
            window_spans: Vec::new(),
            track_summaries,
        };
        trace.refresh_window();
        trace
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

fn load_config() -> (PersistedState, Option<PathBuf>) {
    let Some(project_dirs) = ProjectDirs::from("dev", "tachyon", "Tachyon") else {
        return (PersistedState::default(), None);
    };
    let path = project_dirs.config_dir().join("sessions.json");
    let config = fs::read_to_string(&path)
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    (config, Some(path))
}

fn load_logo_texture(ctx: &egui::Context) -> Option<TextureHandle> {
    let image = image::load_from_memory(LOGO_PNG).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Some(ctx.load_texture("tachyon-logo", color_image, Default::default()))
}

fn load_icon() -> Option<egui::IconData> {
    let image = image::load_from_memory(LOGO_PNG).ok()?.to_rgba8();
    let width = image.width();
    let height = image.height();
    Some(egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

fn line_layout_job(text: &str, line_start: u64, hits: &[&SearchHit], ui: &egui::Ui) -> LayoutJob {
    let mut job = LayoutJob::default();
    let font_id = TextStyle::Monospace.resolve(ui.style());
    let base = TextFormat {
        font_id: font_id.clone(),
        color: ui.visuals().text_color(),
        ..Default::default()
    };
    let highlight = TextFormat {
        font_id,
        color: Color32::BLACK,
        background: Color32::from_rgb(250, 204, 21),
        ..Default::default()
    };

    let mut ranges = hits
        .iter()
        .filter_map(|hit| {
            let start = hit.byte_range.start.0.saturating_sub(line_start) as usize;
            let end = hit.byte_range.end.0.saturating_sub(line_start) as usize;
            (start < end && end <= text.len()).then_some(start..end)
        })
        .collect::<Vec<_>>();
    ranges.sort_by_key(|range| range.start);

    let mut cursor = 0usize;
    for range in ranges {
        if !text.is_char_boundary(range.start) || !text.is_char_boundary(range.end) {
            continue;
        }
        if cursor < range.start {
            job.append(&text[cursor..range.start], 0.0, base.clone());
        }
        job.append(&text[range.start..range.end], 0.0, highlight.clone());
        cursor = range.end;
    }
    if cursor < text.len() {
        job.append(&text[cursor..], 0.0, base);
    }
    job
}

#[allow(dead_code)]
fn search_mode_label(query: &SearchQuery) -> &'static str {
    match query.mode {
        SearchMode::Substring { .. } => "substring",
        SearchMode::Regex => "regex",
    }
}

fn _path_label(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .or_else(|| path.to_str())
        .unwrap_or("file")
}

fn setup_custom_styles(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();

    // Zed-inspired slate palette
    visuals.panel_fill = Color32::from_rgb(15, 23, 42); // Slate 950
    visuals.window_fill = Color32::from_rgb(22, 28, 45); // Slightly lighter for sidebar/windows
    visuals.widgets.noninteractive.bg_fill = visuals.panel_fill;
    visuals.widgets.noninteractive.bg_stroke =
        egui::Stroke::new(1.0, Color32::from_rgb(30, 41, 59)); // Slate 800

    visuals.widgets.inactive.bg_fill = Color32::from_rgb(30, 41, 59); // Slate 800
    visuals.widgets.inactive.rounding = egui::Rounding::same(6.0);

    visuals.widgets.hovered.bg_fill = Color32::from_rgb(51, 65, 85); // Slate 700
    visuals.widgets.hovered.rounding = egui::Rounding::same(6.0);

    visuals.widgets.active.bg_fill = Color32::from_rgb(56, 189, 248); // Sky 400
    visuals.widgets.active.rounding = egui::Rounding::same(6.0);

    visuals.selection.bg_fill = Color32::from_rgb(56, 189, 248).gamma_multiply(0.3);

    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(10.0, 6.0);
    ctx.set_style(style);
}
