#![expect(
    clippy::print_stderr,
    reason = "stage reports connection failures on stderr before the window opens"
)]
#![deny(unsafe_code)]

use eframe::egui::{self, ViewportBuilder, ViewportId};
use ene_api::{ApiClient, CreateSessionRequest, HistoryResponse, MessageMode, MessageRequest};

fn main() {
    let url = std::env::var("ENE_API_URL").unwrap_or_else(|_| "http://127.0.0.1:0".to_owned());
    let token = std::env::var("ENE_API_TOKEN").unwrap_or_default();
    let native = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("ene stage")
            .with_inner_size([480.0, 640.0]),
        ..Default::default()
    };
    if let Err(err) = eframe::run_native(
        "ene stage",
        native,
        Box::new(move |cc| Ok(Box::new(StageApp::new(cc, url, token)))),
    ) {
        eprintln!("ene-stage: {err}");
        std::process::exit(1);
    }
}

struct StageApp {
    client: ApiClient,
    session: Option<String>,
    draft: String,
    surface: Vec<String>,
    detail: Vec<String>,
    error: Option<String>,
    runtime: tokio::runtime::Runtime,
}

impl StageApp {
    fn new(_cc: &eframe::CreationContext<'_>, url: String, token: String) -> Self {
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime,
            Err(err) => {
                eprintln!("ene-stage runtime: {err}");
                std::process::exit(1);
            }
        };
        let client = ApiClient::new(url, token, "stage");
        let mut app = Self {
            client,
            session: None,
            draft: String::new(),
            surface: Vec::new(),
            detail: Vec::new(),
            error: None,
            runtime,
        };
        app.bootstrap();
        app
    }

    fn bootstrap(&mut self) {
        let client = self.client.clone();
        match self.runtime.block_on(async move {
            let health = client.health().await?;
            let souls = client.list_souls().await?;
            let soul_id = if let Some(soul) = souls.items.first() {
                soul.id.clone()
            } else {
                return Ok((health.bind, None::<String>, None::<HistoryResponse>));
            };
            let sessions = client.list_sessions(Some(&soul_id)).await?;
            let session = if let Some(existing) = sessions.items.first() {
                existing.id.clone()
            } else {
                client
                    .create_session(&CreateSessionRequest {
                        soul_id,
                        title: None,
                    })
                    .await?
                    .id
            };
            let history = client.history(&session, "surface").await.ok();
            Ok::<_, ene_api::ApiError>((health.bind, Some(session), history))
        }) {
            Ok((bind, session, history)) => {
                self.surface.push(format!("connected {bind}"));
                self.session = session;
                if let Some(history) = history {
                    for message in history.messages {
                        self.surface
                            .push(format!("{}: {}", message.role, message.text));
                    }
                }
            }
            Err(err) => self.error = Some(err.to_string()),
        }
    }

    fn send(&mut self) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let text = self.draft.trim().to_owned();
        if text.is_empty() {
            return;
        }
        self.draft.clear();
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
                self.surface = surface
                    .messages
                    .into_iter()
                    .filter(|m| m.role != "inner")
                    .map(|m| format!("{}: {}", m.role, m.text))
                    .collect();
                self.detail = detail
                    .messages
                    .into_iter()
                    .map(|m| format!("{}: {}", m.role, m.text))
                    .collect();
            }
            Err(err) => self.error = Some(err.to_string()),
        }
    }

    fn surface_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Companion");
        if let Some(err) = &self.error {
            ui.colored_label(egui::Color32::RED, err);
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for line in &self.surface {
                ui.label(line);
            }
        });
        ui.horizontal(|ui| {
            let enter = ui.text_edit_singleline(&mut self.draft).lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Send").clicked() || enter {
                self.send();
            }
        });
    }
}

impl eframe::App for StageApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            self.surface_ui(ui);
        });

        let detail = self.detail.clone();
        ui.ctx().show_viewport_immediate(
            ViewportId::from_hash_of("detail"),
            ViewportBuilder::default()
                .with_title("ene detail")
                .with_inner_size([520.0, 640.0]),
            move |ui, _class| {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.heading("Detail");
                    ui.label("Inner, thinking, and tool arguments. Not shown on the stage.");
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for line in &detail {
                            ui.label(line);
                        }
                    });
                });
            },
        );
    }
}
