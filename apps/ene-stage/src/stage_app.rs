use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use eframe::egui::{self, ViewportBuilder, ViewportId};
use ene_api::{
    ApiClient, CreateSessionRequest, JobView, MemoryView, MessageMode, MessageRequest, SoulPatch,
    SoulView, StageView,
};
use serde_json::Value;

use crate::core_spawn::{CoreChild, resolve_connection};
use crate::filter::{
    format_event_line, job_report_matches_soul, live_surface_line, merge_soul_ids,
    surface_event_allowed, surface_history_line,
};
use crate::vrm;

const DETAIL_VIEWPORT: &str = "detail";

#[derive(Clone, Debug, Default)]
struct PadState {
    valence: Option<f32>,
    arousal: Option<f32>,
    dominance: Option<f32>,
    mood_label: String,
}

#[derive(Clone)]
struct CompanionPane {
    soul_id: String,
    session_id: Option<String>,
    soul_label: String,
    body_ref: Option<String>,
    surface_lines: Vec<String>,
    draft: String,
    pad: PadState,
    expression: String,
    viseme: String,
    look_at: String,
}

struct DetailState {
    lines: Vec<String>,
    memories: Vec<MemoryView>,
    jobs: Vec<JobView>,
    body_patch_input: String,
    body_patch_warning: Option<String>,
}

enum LiveEventMsg {
    Surface { pane: usize, value: Value },
    Detail { pane: usize, value: Value },
}

pub struct StageApp {
    client: ApiClient,
    runtime: tokio::runtime::Runtime,
    #[expect(
        dead_code,
        reason = "child process kept until StageApp drops (desktop.core_lifetime=app)"
    )]
    core_child: CoreChild,
    companions: [CompanionPane; 2],
    selected: usize,
    detail: DetailState,
    error: Option<String>,
    bind_label: String,
    text_only: bool,
    vrm_path: Option<PathBuf>,
    vrm_left: vrm::VrmPane,
    vrm_right: vrm::VrmPane,
    gpu_ready: bool,
    event_rx: Option<Receiver<LiveEventMsg>>,
    ws_started: bool,
}

impl StageApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        text_only: bool,
        vrm_path: Option<PathBuf>,
    ) -> Self {
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime,
            Err(err) => {
                eprintln!("ene-stage runtime: {err}");
                std::process::exit(1);
            }
        };
        let (client, core_child, connect_err) = match resolve_connection(&runtime) {
            Ok((url, token, child)) => (ApiClient::new(url, token, "stage"), child, None),
            Err(err) => {
                eprintln!("ene-stage: {err}");
                (
                    ApiClient::new("http://127.0.0.1:1", "", "stage"),
                    CoreChild::empty(),
                    Some(err),
                )
            }
        };
        let mut app = Self {
            client,
            runtime,
            core_child,
            companions: [empty_pane("left"), empty_pane("right")],
            selected: 0,
            detail: DetailState {
                lines: Vec::new(),
                memories: Vec::new(),
                jobs: Vec::new(),
                body_patch_input: String::new(),
                body_patch_warning: None,
            },
            error: connect_err,
            bind_label: String::new(),
            text_only,
            vrm_path: vrm_path.clone(),
            vrm_left: vrm::VrmPane::new(vrm_path.clone()),
            vrm_right: vrm::VrmPane::new(vrm_path),
            gpu_ready: false,
            event_rx: None,
            ws_started: false,
        };
        if !text_only {
            app.init_gpu(cc);
        }
        app.bootstrap();
        app
    }

    fn init_gpu(&mut self, cc: &eframe::CreationContext<'_>) {
        let Some(wgpu) = cc.wgpu_render_state.as_ref() else {
            self.error = Some("wgpu render state unavailable".to_owned());
            return;
        };
        let device = &wgpu.device;
        let queue = &wgpu.queue;
        let format = wgpu.target_format;
        self.vrm_left
            .init_gpu(device, queue, format, self.vrm_path.clone());
        self.vrm_right
            .init_gpu(device, queue, format, self.vrm_path.clone());
        self.gpu_ready = true;
    }

    fn bootstrap(&mut self) {
        if self.error.is_some() {
            return;
        }
        let client = self.client.clone();
        match self.runtime.block_on(async move {
            let health = client.health().await?;
            let souls = client.list_souls().await?;
            let stage = client.stage().await?;
            Ok::<_, ene_api::ApiError>((health, souls, stage))
        }) {
            Ok((health, souls, stage)) => {
                self.bind_label = health.bind;
                self.bootstrap_companions(&souls.items, &stage);
                self.refresh_detail_for_selected();
                self.start_ws_if_needed();
            }
            Err(err) => self.error = Some(err.to_string()),
        }
    }

    fn bootstrap_companions(&mut self, souls: &[SoulView], stage: &StageView) {
        let occupant_ids: Vec<String> = stage
            .occupants
            .iter()
            .map(|occupant| occupant.soul_id.clone())
            .collect();
        let extra_ids: Vec<String> = souls.iter().map(|soul| soul.id.clone()).collect();
        let mut soul_ids = merge_soul_ids(&occupant_ids, &extra_ids);
        if soul_ids.is_empty() {
            self.error = Some("no companions available".to_owned());
            return;
        }
        while soul_ids.len() < 2 {
            if let Some(extra) = extra_ids.iter().find(|id| !soul_ids.contains(id)) {
                soul_ids.push(extra.clone());
            } else {
                break;
            }
        }
        for (idx, soul_id) in soul_ids.iter().take(2).enumerate() {
            let soul_meta = souls.iter().find(|s| s.id == *soul_id);
            let label = soul_meta.map_or_else(
                || short_label(soul_id, "new"),
                |s| companion_label(&s.id, &s.display_name, &s.character_ref, &s.mood_label),
            );
            let mood = soul_meta.map_or_else(|| "—".to_owned(), |s| s.mood_label.clone());
            let body_ref = soul_meta.and_then(|s| s.body_ref.clone());
            let session = self.ensure_session(soul_id);
            let surface_lines = if let Some(session_id) = session.as_ref() {
                self.load_surface_history(session_id)
            } else {
                Vec::new()
            };
            self.companions[idx] = CompanionPane {
                soul_id: soul_id.clone(),
                session_id: session,
                soul_label: label,
                body_ref,
                surface_lines,
                draft: String::new(),
                pad: PadState {
                    mood_label: mood.clone(),
                    ..PadState::default()
                },
                expression: mood,
                viseme: "—".to_owned(),
                look_at: "user".to_owned(),
            };
        }
        self.detail.body_patch_input = self
            .companions
            .get(self.selected)
            .and_then(|c| c.body_ref.clone())
            .unwrap_or_default();
    }

    fn ensure_session(&mut self, soul_id: &str) -> Option<String> {
        let client = self.client.clone();
        let soul = soul_id.to_owned();
        match self.runtime.block_on(async move {
            let sessions = client.list_sessions(Some(&soul)).await?;
            if let Some(existing) = sessions.items.first() {
                Ok::<String, ene_api::ApiError>(existing.id.clone())
            } else {
                let created = client
                    .create_session(&CreateSessionRequest {
                        soul_id: soul,
                        title: None,
                    })
                    .await?;
                Ok(created.id)
            }
        }) {
            Ok(id) => Some(id),
            Err(err) => {
                self.error = Some(err.to_string());
                None
            }
        }
    }

    fn load_surface_history(&mut self, session_id: &str) -> Vec<String> {
        let client = self.client.clone();
        let session = session_id.to_owned();
        match self
            .runtime
            .block_on(async move { client.history(&session, "surface").await })
        {
            Ok(history) => history
                .messages
                .into_iter()
                .filter_map(|m| surface_history_line(&m.role, &m.text))
                .collect(),
            Err(err) => {
                self.error = Some(err.to_string());
                Vec::new()
            }
        }
    }

    fn start_ws_if_needed(&mut self) {
        if self.ws_started {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.event_rx = Some(rx);
        self.ws_started = true;
        for (idx, pane) in self.companions.iter().enumerate() {
            if let Some(session_id) = pane.session_id.clone() {
                spawn_ws(
                    self.runtime.handle(),
                    self.client.clone(),
                    session_id,
                    idx,
                    tx.clone(),
                );
            }
        }
    }

    fn drain_events(&mut self) {
        let mut batch = Vec::new();
        if let Some(rx) = self.event_rx.as_ref() {
            while let Ok(msg) = rx.try_recv() {
                batch.push(msg);
            }
        }
        for msg in batch {
            match msg {
                LiveEventMsg::Surface { pane, value } => {
                    if !surface_event_allowed(&value)
                        || !job_report_matches_soul(&value, &self.companions[pane].soul_id)
                    {
                        continue;
                    }
                    self.apply_live_value(pane, &value, false);
                    if let Some(line) = live_surface_line(&value) {
                        self.companions[pane].surface_lines.push(line);
                    }
                }
                LiveEventMsg::Detail { pane, value } => {
                    self.apply_live_value(pane, &value, true);
                    self.detail.lines.push(format_event_line(&value));
                }
            }
        }
    }

    fn apply_live_value(&mut self, pane: usize, value: &Value, detail: bool) {
        let event_type = value.get("type").and_then(Value::as_str);
        if event_type == Some("affect.state") {
            let pad = &mut self.companions[pane].pad;
            if let Some(v) = value.get("valence").and_then(Value::as_f64) {
                pad.valence = Some(v as f32);
            }
            if let Some(v) = value.get("arousal").and_then(Value::as_f64) {
                pad.arousal = Some(v as f32);
            }
            if let Some(v) = value.get("dominance").and_then(Value::as_f64) {
                pad.dominance = Some(v as f32);
            }
            if let Some(label) = value.get("mood_label").and_then(Value::as_str) {
                label.clone_into(&mut pad.mood_label);
                label.clone_into(&mut self.companions[pane].expression);
            }
        }
        if event_type == Some("body.expression")
            && let Some(label) = value.get("label").and_then(Value::as_str)
        {
            label.clone_into(&mut self.companions[pane].expression);
        }
        if matches!(event_type, Some("body.lipsync" | "body.viseme"))
            && let Some(v) = value.get("viseme").and_then(Value::as_str)
        {
            v.clone_into(&mut self.companions[pane].viseme);
        }
        if event_type == Some("body.look_at")
            && let Some(target) = value.get("target").and_then(Value::as_str)
        {
            target.clone_into(&mut self.companions[pane].look_at);
        }
        if detail && matches!(event_type, Some("job.progress" | "job.completed")) {
            self.refresh_jobs();
        }
        self.sync_vrm_labels(pane);
    }

    fn sync_vrm_labels(&mut self, pane: usize) {
        let companion = &self.companions[pane];
        let vrm = if pane == 0 {
            &mut self.vrm_left
        } else {
            &mut self.vrm_right
        };
        vrm.set_performance_labels(&companion.expression, &companion.viseme, &companion.look_at);
    }

    fn refresh_detail_for_selected(&mut self) {
        let soul_id = self.companions[self.selected].soul_id.clone();
        let session_id = self.companions[self.selected].session_id.clone();
        let body_ref = self.companions[self.selected].body_ref.clone();
        self.detail.body_patch_input = body_ref.unwrap_or_default();
        self.detail.body_patch_warning = None;
        self.refresh_memories(&soul_id);
        self.refresh_jobs();
        if let Some(session_id) = session_id {
            self.refresh_detail_history(&session_id);
            self.refresh_soul_pad(&soul_id);
        }
    }

    fn refresh_detail_history(&mut self, session_id: &str) {
        let client = self.client.clone();
        let session = session_id.to_owned();
        match self
            .runtime
            .block_on(async move { client.history(&session, "detail").await })
        {
            Ok(history) => {
                self.detail.lines = history
                    .messages
                    .into_iter()
                    .map(|m| format!("{}: {}", m.role, m.text))
                    .collect();
            }
            Err(err) => self.error = Some(err.to_string()),
        }
    }

    fn refresh_memories(&mut self, soul_id: &str) {
        let client = self.client.clone();
        let soul = soul_id.to_owned();
        match self
            .runtime
            .block_on(async move { client.list_memories(&soul, None).await })
        {
            Ok(page) => self.detail.memories = page.items,
            Err(err) => self.error = Some(err.to_string()),
        }
    }

    fn refresh_jobs(&mut self) {
        let client = self.client.clone();
        match self
            .runtime
            .block_on(async move { client.list_jobs(None).await })
        {
            Ok(page) => self.detail.jobs = page.items,
            Err(err) => self.error = Some(err.to_string()),
        }
    }

    fn refresh_soul_pad(&mut self, soul_id: &str) {
        let client = self.client.clone();
        let soul = soul_id.to_owned();
        let affect = self
            .runtime
            .block_on(async move { client.soul_affect(&soul).await });
        let Ok(affect) = affect else {
            return;
        };
        let Some(pane_idx) = self.companions.iter().position(|c| c.soul_id == soul_id) else {
            return;
        };
        let pane = &mut self.companions[pane_idx];
        pane.pad.valence = Some(affect.valence);
        pane.pad.arousal = Some(affect.arousal);
        pane.pad.dominance = Some(affect.dominance);
        pane.pad.mood_label.clone_from(&affect.mood_label);
        pane.expression.clone_from(&affect.mood_label);
        self.sync_vrm_labels(pane_idx);
    }

    fn send_pane(&mut self, idx: usize) {
        self.selected = idx;
        let Some(session) = self.companions[idx].session_id.clone() else {
            return;
        };
        let text = self.companions[idx].draft.trim().to_owned();
        if text.is_empty() {
            return;
        }
        self.companions[idx].draft.clear();
        let client = self.client.clone();
        match self.runtime.block_on(async move {
            client
                .send_message(
                    &session,
                    &MessageRequest {
                        text: text.clone(),
                        mode: MessageMode::Prompt,
                        input_modality: None,
                    },
                    None,
                )
                .await?;
            let surface = client.history(&session, "surface").await?;
            let detail = client.history(&session, "detail").await?;
            Ok::<_, ene_api::ApiError>((surface, detail))
        }) {
            Ok((surface, detail)) => {
                let soul_id = self.companions[idx].soul_id.clone();
                self.companions[idx].surface_lines = surface
                    .messages
                    .into_iter()
                    .filter_map(|m| surface_history_line(&m.role, &m.text))
                    .collect();
                self.detail.lines = detail
                    .messages
                    .into_iter()
                    .map(|m| format!("{}: {}", m.role, m.text))
                    .collect();
                self.refresh_soul_pad(&soul_id);
                self.refresh_memories(&soul_id);
                self.refresh_jobs();
            }
            Err(err) => self.error = Some(err.to_string()),
        }
    }

    fn patch_body(&mut self) {
        let pane = &mut self.companions[self.selected];
        let soul_id = pane.soul_id.clone();
        let body_ref = self.detail.body_patch_input.trim();
        let patch = SoulPatch {
            body_ref: if body_ref.is_empty() {
                None
            } else {
                Some(body_ref.to_owned())
            },
        };
        let client = self.client.clone();
        match self
            .runtime
            .block_on(async move { client.patch_soul_body(&soul_id, &patch).await })
        {
            Ok(soul) => {
                pane.body_ref = soul.body_ref;
                self.detail.body_patch_warning = None;
            }
            Err(err) => {
                let msg = err.to_string();
                self.detail.body_patch_warning = if msg.to_ascii_lowercase().contains("compatib") {
                    Some(msg.clone())
                } else {
                    None
                };
                self.error = Some(msg);
            }
        }
    }

    fn companion_ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame, idx: usize) {
        ui.push_id(idx, |ui| {
            let selected = self.selected == idx;
            if ui
                .selectable_label(selected, format!("Companion {}", idx + 1))
                .clicked()
            {
                self.selected = idx;
                self.refresh_detail_for_selected();
            }
            let soul_label = self.companions[idx].soul_label.clone();
            ui.label(format!("soul {soul_label}"));
            if !self.text_only && self.gpu_ready {
                self.draw_vrm_slot(ui, frame, idx);
            } else if !self.text_only {
                ui.label("3D unavailable (text-only or missing GPU)");
            }
            let overlay = format!(
                "body slot: expr={} viseme={} look={}",
                self.companions[idx].expression,
                self.companions[idx].viseme,
                self.companions[idx].look_at,
            );
            ui.small(overlay);
            let lines = self.companions[idx].surface_lines.clone();
            egui::ScrollArea::vertical()
                .id_salt(format!("surface-{idx}"))
                .min_scrolled_height(96.0)
                .max_height(180.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if lines.is_empty() {
                        ui.weak("no speech yet");
                    }
                    for line in &lines {
                        ui.label(line);
                    }
                });
            let mut send = false;
            ui.horizontal(|ui| {
                let reply = ui.text_edit_singleline(&mut self.companions[idx].draft);
                send = ui.button("Send").clicked()
                    || (reply.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            });
            if send {
                self.send_pane(idx);
            }
        });
    }

    fn draw_vrm_slot(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame, idx: usize) {
        let desired = egui::vec2(ui.available_width().clamp(140.0, 360.0), 180.0);
        let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
        let Some(wgpu) = frame.wgpu_render_state() else {
            ui.put(rect, egui::Label::new("wgpu unavailable"));
            return;
        };
        let width = rect.width().max(1.0) as u32;
        let height = rect.height().max(1.0) as u32;
        let device = &wgpu.device;
        let queue = &wgpu.queue;
        let format = wgpu.target_format;
        let mut renderer = wgpu.renderer.write();
        let vrm = if idx == 0 {
            &mut self.vrm_left
        } else {
            &mut self.vrm_right
        };
        vrm.ensure_targets(device, &mut renderer, width, height, format);
        vrm.render_frame(device, queue);
        if let Some(err) = vrm.load_error() {
            ui.put(rect, egui::Label::new(err));
        } else if let Some(texture_id) = vrm.texture_id() {
            ui.put(
                rect,
                egui::Image::new((texture_id, egui::vec2(rect.width(), rect.height()))),
            );
        } else {
            ui.put(rect, egui::Label::new(vrm.overlay_text()));
        }
        ui.small(vrm.overlay_text());
    }

    fn surface_ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        ui.heading("Stage");
        ui.label(format!("connected {}", self.bind_label));
        if let Some(err) = &self.error {
            ui.colored_label(egui::Color32::RED, err);
        }
        ui.columns(2, |columns| {
            self.companion_ui(&mut columns[0], frame, 0);
            self.companion_ui(&mut columns[1], frame, 1);
        });
    }

    fn detail_panel(&mut self, ui: &mut egui::Ui) {
        let pane = &self.companions[self.selected];
        ui.heading("Detail");
        ui.label("Inner, thinking, tasks, memories, and PAD affect — not on the stage.");
        ui.separator();
        ui.label(format!(
            "PAD (soul {}): mood={} valence={} arousal={} dominance={}",
            pane.soul_label,
            pane.pad.mood_label,
            fmt_pad(pane.pad.valence),
            fmt_pad(pane.pad.arousal),
            fmt_pad(pane.pad.dominance),
        ));
        ui.separator();
        ui.label("Soul / body mix");
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.detail.body_patch_input);
            if ui.button("PATCH body").clicked() {
                self.patch_body();
            }
        });
        if let Some(warn) = &self.detail.body_patch_warning {
            ui.colored_label(egui::Color32::YELLOW, warn);
        }
        ui.separator();
        ui.label("Memories");
        for memory in &self.detail.memories {
            ui.label(format!(
                "[{}] {}: {}",
                memory.scope, memory.title, memory.content
            ));
        }
        ui.separator();
        ui.label("Tasks");
        for job in &self.detail.jobs {
            ui.label(format!(
                "{} {} ({}) {}",
                job.status,
                job.title,
                job.soul_id,
                job.progress_note.clone().unwrap_or_default()
            ));
        }
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for line in &self.detail.lines {
                ui.label(line);
            }
        });
    }
}

impl eframe::App for StageApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.drain_events();
        egui::CentralPanel::default().show(ui, |ui| {
            self.surface_ui(ui, frame);
        });

        ui.ctx().show_viewport_immediate(
            ViewportId::from_hash_of(DETAIL_VIEWPORT),
            ViewportBuilder::default()
                .with_title("ene detail")
                .with_inner_size([520.0, 720.0]),
            |detail_ui, _class| {
                egui::CentralPanel::default().show(detail_ui, |ui| {
                    self.detail_panel(ui);
                });
            },
        );
    }
}

fn empty_pane(side: &str) -> CompanionPane {
    CompanionPane {
        soul_id: String::new(),
        session_id: None,
        soul_label: side.to_owned(),
        body_ref: None,
        surface_lines: Vec::new(),
        draft: String::new(),
        pad: PadState::default(),
        expression: "neutral".to_owned(),
        viseme: "—".to_owned(),
        look_at: "user".to_owned(),
    }
}

fn short_label(id: &str, mood: &str) -> String {
    let short = id.chars().take(8).collect::<String>();
    format!("{short} ({mood})")
}

/// Prefer package names: ULID prefixes collide when two souls are minted together.
pub(crate) fn companion_label(
    id: &str,
    display_name: &str,
    character_ref: &str,
    mood: &str,
) -> String {
    let name = [display_name, character_ref]
        .into_iter()
        .find(|value| !value.is_empty());
    match name {
        Some(name) => format!("{name} ({mood})"),
        None => short_label(id, mood),
    }
}

fn fmt_pad(value: Option<f32>) -> String {
    match value {
        Some(v) => format!("{v:.2}"),
        None => "—".to_owned(),
    }
}

fn spawn_ws(
    handle: &tokio::runtime::Handle,
    client: ApiClient,
    session_id: String,
    pane: usize,
    tx: std::sync::mpsc::Sender<LiveEventMsg>,
) {
    handle.spawn(async move {
        let mut surface = match client.events("surface", Some(&session_id)).await {
            Ok(sock) => sock,
            Err(err) => {
                eprintln!("ene-stage surface ws: {err}");
                return;
            }
        };
        let mut detail = match client.events("detail", Some(&session_id)).await {
            Ok(sock) => sock,
            Err(err) => {
                eprintln!("ene-stage detail ws: {err}");
                return;
            }
        };
        loop {
            let surface_next = surface.recv_json();
            let detail_next = detail.recv_json();
            let (surface_msg, detail_msg) = tokio::join!(surface_next, detail_next);
            match surface_msg {
                Ok(Some(value)) => {
                    if tx.send(LiveEventMsg::Surface { pane, value }).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    eprintln!("ene-stage surface ws read: {err}");
                    break;
                }
            }
            match detail_msg {
                Ok(Some(value)) => {
                    if tx.send(LiveEventMsg::Detail { pane, value }).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    eprintln!("ene-stage detail ws read: {err}");
                    break;
                }
            }
        }
    });
}
