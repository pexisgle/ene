use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use std::time::Duration;

use ene_api::v1::management::ManagementOutcome;
use ene_config::{Config, resolve_data_dir};
use ene_desktop::i18n::Locale;
use ene_desktop::measure::{InteractionSample, InteractionTraceLine, monotonic_ns};
use ene_desktop::session::{ChatIntakeRefusal, ChatSendReport, ChatStreamEnd};
use ene_desktop::ui::presentation::{SurfaceSnapshot, parse_cap};
use ene_desktop::ui::{DesktopError, DesktopRuntime};
use ene_desktop_ui::{ChatWindow, Item, ManagementWindow, Message};
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use tokio::sync::{Notify, oneshot};
use zeroize::Zeroizing;

enum Command {
    Startup,
    Refresh(i32),
    History,
    Tasks,
    SelectTask(String),
    Send(String),
    Workspace(String),
    Resume(String, String),
    CancelTask(String),
    ShowBody,
    HideBody,
    Locale(bool),
    Next,
    Back,
    Secret(Zeroizing<String>),
    Assign(String),
    Memory(String),
    MoreMemory,
    MoreRevisions,
    MoreUsage,
    Cap(u64),
    SelectDeletion(String),
    RequestDeletion(String),
    BeginDeletion(String),
    ResumeDeletion(String),
    Confirm(String),
    Dismiss,
}
impl Command {
    fn management_page(&self) -> Option<i32> {
        match self {
            Self::Refresh(page) => Some(*page),
            Self::ShowBody | Self::HideBody => Some(0),
            Self::Startup | Self::Next | Self::Back | Self::Secret(_) | Self::Assign(_) => Some(6),
            Self::Memory(_) | Self::MoreMemory | Self::MoreRevisions => Some(2),
            Self::MoreUsage | Self::Cap(_) => Some(3),
            Self::SelectDeletion(_)
            | Self::RequestDeletion(_)
            | Self::BeginDeletion(_)
            | Self::ResumeDeletion(_) => Some(4),
            Self::Confirm(_) => Some(7),
            _ => None,
        }
    }

    fn measured_operation(&self) -> Option<&'static str> {
        match self {
            Self::CancelTask(_) => Some(ene_desktop::measure::CANCEL_TASK_OPERATION),
            _ => None,
        }
    }
}

struct CommandOutput {
    notice: String,
    failed: bool,
    consume_sent_draft: bool,
}

impl CommandOutput {
    fn info(notice: impl Into<String>) -> Self {
        Self {
            notice: notice.into(),
            failed: false,
            consume_sent_draft: false,
        }
    }

    fn error(notice: impl Into<String>) -> Self {
        Self {
            notice: notice.into(),
            failed: true,
            consume_sent_draft: false,
        }
    }

    fn consumed_info(notice: impl Into<String>) -> Self {
        Self {
            notice: notice.into(),
            failed: false,
            consume_sent_draft: true,
        }
    }
}

#[derive(Clone, Copy)]
struct InteractionStart {
    operation: &'static str,
    input_monotonic_ns: u64,
}
struct PendingPaint {
    start: InteractionStart,
    host_outcome_monotonic_ns: u64,
}
struct Request {
    command: Command,
    lane: usize,
    management_page: Option<i32>,
    epoch: u64,
    generation: u64,
    interaction: Option<InteractionStart>,
}
#[derive(Default)]
struct Mailbox {
    queue: Mutex<VecDeque<Request>>,
    wake: Notify,
    epoch: AtomicU64,
    generation: AtomicU64,
    pending: [AtomicUsize; 4],
}
impl Mailbox {
    fn push(&self, command: Command, lane: usize) -> bool {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if queue.len() >= 16 {
            return false;
        }
        self.pending[lane].fetch_add(1, Ordering::SeqCst);
        let interaction = command
            .measured_operation()
            .map(|operation| InteractionStart {
                operation,
                input_monotonic_ns: monotonic_ns(),
            });
        queue.push_back(Request {
            management_page: command.management_page(),
            command,
            lane,
            epoch: self.epoch.load(Ordering::SeqCst),
            generation: self.generation.load(Ordering::SeqCst),
            interaction,
        });
        self.wake.notify_one();
        true
    }
    fn pop(&self) -> Option<Request> {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
    }
    fn complete(&self, lane: usize, epoch: u64) {
        let _queue = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if epoch == self.epoch.load(Ordering::SeqCst) {
            self.pending[lane].fetch_sub(1, Ordering::SeqCst);
        }
    }
    fn erase(&self) {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        queue.clear();
        for pending in &self.pending {
            pending.store(0, Ordering::SeqCst);
        }
    }
}
#[derive(Clone)]
struct Surfaces {
    chat: slint::Weak<ChatWindow>,
    management: slint::Weak<ManagementWindow>,
    mailbox: Arc<Mailbox>,
    pending_paints: Arc<Mutex<VecDeque<PendingPaint>>>,
    interaction_paint_evidence: bool,
}
impl Surfaces {
    fn submit(&self, command: Command, lane: usize) -> bool {
        let (Some(chat), Some(management)) = (self.chat.upgrade(), self.management.upgrade())
        else {
            return false;
        };
        let busy = lane != 0 && self.mailbox.pending[lane].load(Ordering::SeqCst) > 0;
        if busy || !self.mailbox.push(command, lane) {
            backpressure_notice(&management);
            return false;
        }
        match lane {
            1 => {
                chat.set_busy(true);
                chat.set_notice("".into());
            }
            2 => {
                chat.set_task_busy(true);
                chat.set_task_notice("".into());
            }
            3 => {
                management.set_busy(true);
                management.set_notice("".into());
            }
            _ => {}
        }
        true
    }
    fn dismiss(&self) {
        self.mailbox.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(m) = self.management.upgrade() {
            ene_desktop::ui::presentation::discard_secret_input(&m);
            m.set_can_confirm(false);
            m.set_confirmation_key("".into());
            m.set_confirmation_target("".into());
            m.set_confirmation_title("".into());
            m.set_confirmation_description("".into());
            if m.get_page() == 7 {
                m.set_page(6);
            }
        }
        if !self.mailbox.push(Command::Dismiss, 0)
            && let Some(m) = self.management.upgrade()
        {
            backpressure_notice(&m);
        }
    }
}

fn backpressure_notice(management: &ManagementWindow) {
    management.set_notice(
        local(
            management.get_japanese(),
            "操作が混み合っています。少し待ってください。",
            "Too many pending actions. Please wait.",
        )
        .into(),
    );
}

pub fn run() -> Result<(), DesktopError> {
    let config = Config::load(None).map_err(|e| DesktopError::Protocol(e.to_string()))?;
    let data_dir = resolve_data_dir(&config)
        .ok_or_else(|| DesktopError::HostLaunch("no data directory resolved".into()))?;
    match std::env::var(ene_local_control::CONFIRMATION_MODE_ENV) {
        Ok(value) if value == ene_local_control::CONFIRMATION_MODE_STDIO => run_gui(data_dir),
        _ => run_launcher(data_dir),
    }
}

fn run_launcher(data_dir: std::path::PathBuf) -> Result<(), DesktopError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| DesktopError::Transport(e.to_string()))?;
    runtime.block_on(async move {
        ene_desktop::host_launch::ensure_serving(&data_dir)
            .map_err(|error| DesktopError::HostLaunch(error.to_string()))?;
        let requester = ene_desktop::control::RequesterClient::new(&data_dir);
        let mut attempts = 0_u8;
        loop {
            match requester.open_desktop().await {
                Ok(true) => return Ok(()),
                Ok(false) => {
                    return Err(DesktopError::HostLaunch(String::from(
                        "the Host could not start its GUI",
                    )));
                }
                Err(error) => {
                    if !launcher_retries(&error) {
                        return Err(error);
                    }
                    attempts = attempts.saturating_add(1);
                    if attempts >= ene_desktop::session::BOOTSTRAP_ATTEMPTS {
                        return Err(error);
                    }
                    tokio::time::sleep(ene_desktop::session::BOOTSTRAP_DELAY).await;
                }
            }
        }
    })
}

fn launcher_retries(error: &DesktopError) -> bool {
    !matches!(error, DesktopError::BackpressureHold)
}

fn run_gui(data_dir: std::path::PathBuf) -> Result<(), DesktopError> {
    slint::BackendSelector::new()
        .backend_name(String::from("winit"))
        .renderer_name(String::from("software"))
        .select()
        .map_err(platform_error)?;
    let channel = ene_local_control::GuiChannel::adopt_stdio()
        .map_err(|error| DesktopError::Transport(format!("confirmation channel: {error}")))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| DesktopError::Transport(e.to_string()))?;
    let mut desktop = DesktopRuntime::new(data_dir);
    desktop.attach_confirmation(channel)?;
    let chat = ChatWindow::new().map_err(platform_error)?;
    let management = ManagementWindow::new().map_err(platform_error)?;
    chat.window().set_size(slint::LogicalSize::new(1100., 760.));
    management
        .window()
        .set_size(slint::LogicalSize::new(1000., 720.));
    let mut surfaces = Surfaces {
        chat: chat.as_weak(),
        management: management.as_weak(),
        mailbox: Arc::default(),
        pending_paints: Arc::default(),
        interaction_paint_evidence: false,
    };
    surfaces.interaction_paint_evidence = install_interaction_notifier(&chat, &surfaces);
    let initial = desktop.surface_snapshot();
    chat.set_japanese(initial.japanese);
    management.set_japanese(initial.japanese);
    chat.set_status(initial.status.as_str().into());
    management.set_status(initial.status.as_str().into());
    bind(&surfaces, &chat, &management);
    attach_erasure(&mut desktop, surfaces.clone());
    management.show().map_err(platform_error)?;
    request_post_show_redraw(&management);
    surfaces.submit(Command::Startup, 3);
    runtime.spawn(worker(desktop, surfaces));
    let result = slint::run_event_loop().map_err(platform_error);
    runtime.shutdown_timeout(Duration::from_secs(1));
    result
}
fn platform_error(e: slint::PlatformError) -> DesktopError {
    DesktopError::Protocol(e.to_string())
}

fn install_interaction_notifier(chat: &ChatWindow, surfaces: &Surfaces) -> bool {
    let pending = Arc::clone(&surfaces.pending_paints);
    let Some(trace_path) =
        std::env::var_os("ENE_INTERACTION_TRACE_JSONL").map(std::path::PathBuf::from)
    else {
        return false;
    };
    let completion_scheduled = Arc::new(AtomicBool::new(false));
    chat.window().on_winit_window_event(move |_window, event| {
        if matches!(event, winit::event::WindowEvent::RedrawRequested) {
            let has_pending = !pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty();
            if has_pending && !completion_scheduled.swap(true, Ordering::SeqCst) {
                let pending = Arc::clone(&pending);
                let trace_path = trace_path.clone();
                let scheduled_for_completion = Arc::clone(&completion_scheduled);
                let queued = slint::invoke_from_event_loop(move || {
                    let paints = drain_painted_interactions(&pending, monotonic_ns());
                    for sample in paints {
                        append_interaction_trace(&trace_path, sample);
                    }
                    scheduled_for_completion.store(false, Ordering::SeqCst);
                });
                if queued.is_err() {
                    completion_scheduled.store(false, Ordering::SeqCst);
                }
            }
        }
        EventResult::Propagate
    });
    true
}

fn drain_painted_interactions(
    pending: &Mutex<VecDeque<PendingPaint>>,
    painted_monotonic_ns: u64,
) -> Vec<InteractionSample> {
    pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .drain(..)
        .map(|paint| InteractionSample {
            operation: paint.start.operation.to_string(),
            input_monotonic_ns: paint.start.input_monotonic_ns,
            host_outcome_monotonic_ns: paint.host_outcome_monotonic_ns,
            gui_painted_monotonic_ns: painted_monotonic_ns,
        })
        .collect()
}

fn append_interaction_trace(path: &std::path::Path, sample: InteractionSample) {
    let Ok(since_epoch) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
        return;
    };
    let Ok(observed_unix_ns) = u64::try_from(since_epoch.as_nanos()) else {
        return;
    };
    let line = InteractionTraceLine {
        observed_unix_ns,
        sample,
    };
    let Ok(mut encoded) = serde_json::to_vec(&line) else {
        return;
    };
    encoded.push(b'\n');
    let opened = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path);
    let Ok(mut file) = opened else {
        return;
    };
    let _result = std::io::Write::write_all(&mut file, &encoded);
}
fn show<C: ComponentHandle + 'static>(window: &C) {
    match window.show() {
        Ok(_) | Err(_) => {}
    }
    window.window().set_minimized(false);
    window.window().with_winit_window(|w| w.focus_window());
    request_post_show_redraw(window);
}

fn request_post_show_redraw<C: ComponentHandle + 'static>(window: &C) {
    window.window().request_redraw();
    window
        .window()
        .with_winit_window(|window| window.request_redraw());
    let weak = window.as_weak();
    slint::Timer::single_shot(Duration::from_millis(32), move || {
        if let Some(window) = weak.upgrade() {
            window.window().request_redraw();
            window
                .window()
                .with_winit_window(|window| window.request_redraw());
        }
    });
}
fn bind(s: &Surfaces, c: &ChatWindow, m: &ManagementWindow) {
    c.on_open_management({
        let s = s.clone();
        move || {
            if let Some(m) = s.management.upgrade() {
                show(&m);
            }
        }
    });
    m.on_open_chat({
        let s = s.clone();
        move || {
            if let Some(c) = s.chat.upgrade() {
                show(&c);
            }
        }
    });
    m.window().on_close_requested({
        let s = s.clone();
        move || {
            s.dismiss();
            slint::CloseRequestResponse::HideWindow
        }
    });
    m.on_navigate({
        let s = s.clone();
        move |page| {
            s.dismiss();
            if let Some(m) = s.management.upgrade() {
                m.set_page(page);
                m.set_notice("".into());
            }
            if [0, 1, 2, 3, 4, 6].contains(&page)
                && s.mailbox.push(Command::Refresh(page), 3)
                && let Some(m) = s.management.upgrade()
            {
                m.set_busy(true);
            }
        }
    });
    m.on_refresh({
        let s = s.clone();
        move || {
            if let Some(m) = s.management.upgrade() {
                s.submit(Command::Refresh(m.get_page()), 3);
            }
        }
    });
    m.on_show_avatar({
        let s = s.clone();
        move || {
            s.submit(Command::ShowBody, 3);
        }
    });
    m.on_hide_avatar({
        let s = s.clone();
        move || {
            s.submit(Command::HideBody, 3);
        }
    });
    c.on_refresh_history({
        let s = s.clone();
        move || {
            s.submit(Command::History, 1);
        }
    });
    c.on_refresh_tasks({
        let s = s.clone();
        move || {
            s.submit(Command::Tasks, 2);
        }
    });
    c.on_select_task({
        let s = s.clone();
        move |key| {
            s.submit(Command::SelectTask(key.to_string()), 2);
        }
    });
    c.on_send({
        let s = s.clone();
        move |text| {
            s.submit(Command::Send(text.to_string()), 1);
        }
    });
    c.on_workspace({
        let s = s.clone();
        move |path| {
            s.submit(Command::Workspace(path.to_string()), 2);
        }
    });
    c.on_resume({
        let s = s.clone();
        move |text| {
            if let Some(c) = s.chat.upgrade() {
                s.submit(
                    Command::Resume(c.get_selected_task().to_string(), text.to_string()),
                    2,
                );
            }
        }
    });
    c.on_cancel_task({
        let s = s.clone();
        move || {
            if let Some(c) = s.chat.upgrade() {
                s.submit(Command::CancelTask(c.get_selected_task().to_string()), 2);
            }
        }
    });
    m.on_language({
        let s = s.clone();
        move |ja| {
            if let Some(c) = s.chat.upgrade() {
                c.set_japanese(ja);
            }
            if let Some(m) = s.management.upgrade() {
                m.set_japanese(ja);
            }
            s.submit(Command::Locale(ja), 0);
        }
    });
    m.on_wizard_next({
        let s = s.clone();
        move || {
            if let Some(m) = s.management.upgrade() {
                m.set_step((m.get_step() + 1).min(4));
            }
            s.submit(Command::Next, 0);
        }
    });
    m.on_wizard_back({
        let s = s.clone();
        move || {
            if let Some(m) = s.management.upgrade() {
                m.invoke_clear_secret();
                m.set_step((m.get_step() - 1).max(0));
            }
            s.submit(Command::Back, 0);
        }
    });
    m.on_register_secret({
        let s = s.clone();
        move |key| {
            s.submit(Command::Secret(Zeroizing::new(key.to_string())), 3);
        }
    });
    m.on_assign_model({
        let s = s.clone();
        move |model| {
            s.submit(Command::Assign(model.to_string()), 3);
        }
    });
    m.on_select_memory({
        let s = s.clone();
        move |key| {
            s.submit(Command::Memory(key.to_string()), 3);
        }
    });
    m.on_next_memory({
        let s = s.clone();
        move || {
            s.submit(Command::MoreMemory, 3);
        }
    });
    m.on_next_revisions({
        let s = s.clone();
        move || {
            s.submit(Command::MoreRevisions, 3);
        }
    });
    m.on_next_usage({
        let s = s.clone();
        move || {
            s.submit(Command::MoreUsage, 3);
        }
    });
    m.on_apply_cap({
        let s = s.clone();
        move |value| {
            if let Some(value) = parse_cap(&value) {
                s.submit(Command::Cap(value), 3);
            } else if let Some(m) = s.management.upgrade() {
                m.set_notice(
                    local(
                        m.get_japanese(),
                        "0 以上、小数点以下 6 桁以内の金額を入力してください。",
                        "Enter a nonnegative amount with at most six decimal places.",
                    )
                    .into(),
                );
            }
        }
    });
    m.on_select_deletion({
        let s = s.clone();
        move |key| {
            s.submit(Command::SelectDeletion(key.to_string()), 3);
        }
    });
    m.on_request_deletion({
        let s = s.clone();
        move |text| {
            if s.submit(Command::RequestDeletion(text.to_string()), 3)
                && let Some(m) = s.management.upgrade()
            {
                m.set_deletion_draft("".into());
            }
        }
    });
    m.on_confirm_deletion({
        let s = s.clone();
        move || {
            if let Some(m) = s.management.upgrade() {
                s.submit(
                    Command::BeginDeletion(m.get_selected_deletion().to_string()),
                    3,
                );
            }
        }
    });
    m.on_resume_deletion({
        let s = s.clone();
        move || {
            if let Some(m) = s.management.upgrade() {
                s.submit(
                    Command::ResumeDeletion(m.get_selected_deletion().to_string()),
                    3,
                );
            }
        }
    });
    m.on_cancel_owner({
        let s = s.clone();
        move || {
            s.dismiss();
        }
    });
    m.on_confirm_owner({
        let s = s.clone();
        move || {
            if let Some(m) = s.management.upgrade()
                && m.get_can_confirm()
                && s.submit(Command::Confirm(m.get_confirmation_key().to_string()), 3)
            {
                m.set_can_confirm(false);
            }
        }
    });
}
fn attach_erasure(desktop: &mut DesktopRuntime, surfaces: Surfaces) {
    desktop.attach_surface_erasure(Arc::new(move || {
        let s = surfaces.clone();
        Box::pin(async move {
            s.mailbox.erase();
            let (tx, rx) = oneshot::channel();
            let queued = slint::invoke_from_event_loop(move || {
                s.mailbox.erase();
                let cleared = if let (Some(c), Some(m)) = (s.chat.upgrade(), s.management.upgrade())
                {
                    c.set_busy(false);
                    c.set_task_busy(false);
                    m.set_busy(false);
                    ene_desktop::ui::presentation::erase_surface_copies(&c, &m)
                } else {
                    false
                };
                match tx.send(cleared) {
                    Ok(_) | Err(_) => {}
                }
            })
            .is_ok();
            queued
                && tokio::time::timeout(Duration::from_secs(3), rx)
                    .await
                    .is_ok_and(|r| r.unwrap_or(false))
        })
    }));
}
async fn worker(mut desktop: DesktopRuntime, s: Surfaces) {
    loop {
        if desktop.confirmation_lost() {
            desktop.close_after_confirmation_loss();
            match slint::quit_event_loop() {
                Ok(_) | Err(_) => {}
            }
            return;
        }
        let notified = s.mailbox.wake.notified();
        let Some(request) = s.mailbox.pop() else {
            tokio::select! { ()=notified => {}, ()=tokio::time::sleep(Duration::from_millis(250)) => desktop.tick() }
            continue;
        };
        if request.epoch != s.mailbox.epoch.load(Ordering::SeqCst) {
            continue;
        }
        let startup = matches!(&request.command, Command::Startup);
        let reset_step = matches!(&request.command, Command::Startup | Command::Confirm(_));
        let confirmation_action = matches!(
            &request.command,
            Command::Startup | Command::Secret(_) | Command::BeginDeletion(_) | Command::Refresh(6)
        );
        if matches!(
            &request.command,
            Command::Confirm(_) | Command::Secret(_) | Command::BeginDeletion(_)
        ) && request.generation != s.mailbox.generation.load(Ordering::SeqCst)
        {
            s.mailbox.complete(request.lane, request.epoch);
            continue;
        }
        let sent_text = match &request.command {
            Command::Send(text) => Some(text.clone()),
            _ => None,
        };
        let is_send = sent_text.is_some();
        let result = execute(&mut desktop, request.command).await;
        let measured = if s.interaction_paint_evidence && result.is_ok() {
            request.interaction.map(|start| PendingPaint {
                start,
                host_outcome_monotonic_ns: monotonic_ns(),
            })
        } else {
            None
        };
        s.mailbox.complete(request.lane, request.epoch);
        if request.generation != s.mailbox.generation.load(Ordering::SeqCst) {
            desktop.cancel_secret();
        }
        let snap = desktop.surface_snapshot();
        let surfaces = s.clone();
        let (tx, rx) = oneshot::channel();
        if slint::invoke_from_event_loop(move || {
            if let (Some(c), Some(m)) = (surfaces.chat.upgrade(), surfaces.management.upgrade()) {
                apply(&c, &m, &snap, reset_step);
                if startup {
                    request_post_show_redraw(&m);
                }
                c.set_busy(surfaces.mailbox.pending[1].load(Ordering::SeqCst) > 0);
                c.set_task_busy(surfaces.mailbox.pending[2].load(Ordering::SeqCst) > 0);
                m.set_busy(surfaces.mailbox.pending[3].load(Ordering::SeqCst) > 0);
                let ja = m.get_japanese();
                let (notice, failed, consume_sent_draft) = match result {
                    Ok(output) => (output.notice, output.failed, output.consume_sent_draft),
                    Err(error) => {
                        let (notice, failed) = if is_send {
                            chat_error_notice(ja, &error)
                        } else {
                            result_notice(ja, Err(error))
                        };
                        (notice, failed, is_send)
                    }
                };
                if is_send
                    && consume_sent_draft
                    && sent_text.as_deref() == Some(c.get_draft().as_str())
                {
                    c.set_draft("".into());
                }
                match request.lane {
                    1 => {
                        c.set_notice_error(failed);
                        c.set_notice(notice.into());
                    }
                    2 => {
                        c.set_task_notice_error(failed);
                        c.set_task_notice(notice.into());
                    }
                    _ => {
                        m.set_notice_error(failed);
                        if request.management_page == Some(m.get_page()) {
                            m.set_notice(notice.into());
                        }
                    }
                }
                if let Some(measured) = measured {
                    surfaces
                        .pending_paints
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push_back(measured);
                    c.window().request_redraw();
                }
                if request.generation == surfaces.mailbox.generation.load(Ordering::SeqCst) {
                    if let Some(confirm) = &snap.confirmation {
                        if confirmation_action {
                            m.set_confirmation_key(confirm.key.as_str().into());
                            m.set_confirmation_title(confirm.title.as_str().into());
                            m.set_confirmation_description(confirm.description.as_str().into());
                            m.set_confirmation_target(confirm.target.as_str().into());
                            m.set_can_confirm(true);
                            m.set_page(7);
                            show(&m);
                        }
                    } else if m.get_page() == 7 {
                        m.set_page(6);
                        m.set_can_confirm(false);
                        m.set_confirmation_key("".into());
                    }
                }
            }
            match tx.send(()) {
                Ok(_) | Err(_) => {}
            }
        })
        .is_err()
        {
            return;
        }
        if rx.await.is_err() {
            return;
        }
    }
}
async fn execute(d: &mut DesktopRuntime, command: Command) -> Result<CommandOutput, DesktopError> {
    let ja = d.surface_snapshot().japanese;
    let outcome = match command {
        Command::Startup => {
            d.ensure_host()?;
            d.try_spawn_located_body();
            d.connect_or_begin_pairing().await?;
            None
        }
        Command::Refresh(page) => {
            match page {
                6 if !d.surface_snapshot().connected => d.connect_or_begin_pairing().await?,
                2 => d.refresh_memory().await?,
                3 => d.refresh_usage().await?,
                4 => d.refresh_deletion_requests().await?,
                _ => d.refresh_setup().await?,
            };
            None
        }
        Command::History => {
            d.refresh_history().await?;
            None
        }
        Command::Tasks => {
            d.refresh_tasks().await?;
            None
        }
        Command::SelectTask(key) => {
            d.select_task_key(&key).await?;
            None
        }
        Command::Send(text) => {
            d.composer_mut().set_draft(text);
            let report = d.send_text().await?;
            return Ok(chat_output(ja, report));
        }
        Command::Workspace(path) => Some(
            d.select_workspace_folder(std::path::Path::new(&path))
                .await?,
        ),
        Command::Resume(key, text) => {
            d.check_task_key(&key)?;
            let result = d.resume_displayed_task(text).await?;
            return Ok(CommandOutput::info(resume_notice(ja, &result)));
        }
        Command::CancelTask(key) => {
            d.check_task_key(&key)?;
            Some(d.cancel_displayed_task().await?)
        }
        Command::ShowBody => {
            d.show_body();
            None
        }
        Command::HideBody => {
            d.hide_body();
            None
        }
        Command::Locale(ja) => {
            d.set_locale(if ja { Locale::Ja } else { Locale::En });
            None
        }
        Command::Next => {
            d.wizard_next();
            None
        }
        Command::Back => {
            d.wizard_back();
            None
        }
        Command::Secret(mut secret) => {
            d.set_secret(std::mem::take(&mut *secret));
            d.begin_credential_put().await?;
            None
        }
        Command::Assign(model) => {
            d.set_model(model);
            Some(d.assign_model().await?)
        }
        Command::Memory(key) => {
            d.select_memory_key(&key).await?;
            None
        }
        Command::MoreMemory => {
            d.page_older_memories().await?;
            None
        }
        Command::MoreRevisions => {
            d.page_later_revisions().await?;
            None
        }
        Command::MoreUsage => {
            d.next_usage_page().await?;
            None
        }
        Command::Cap(value) => {
            d.set_usage_cap_limit_micros(value);
            Some(d.apply_usage_cap().await?)
        }
        Command::SelectDeletion(key) => {
            d.select_deletion_key(&key)?;
            None
        }
        Command::RequestDeletion(text) => {
            d.set_deletion_exact_text(text);
            let result = d.request_deletion().await?;
            let _result = d.refresh_deletion_requests().await;
            Some(result)
        }
        Command::BeginDeletion(key) => {
            d.select_deletion_key(&key)?;
            d.begin_deletion_confirm().await?;
            None
        }
        Command::ResumeDeletion(key) => {
            d.select_deletion_key(&key)?;
            let result = d.resume_deletion().await?;
            return Ok(CommandOutput::info(control_notice(ja, &result)));
        }
        Command::Confirm(key) => {
            let result = d.confirm_key(&key).await?;
            if matches!(
                &result,
                ene_local_control::FromConfirmation::Outcome(
                    ene_local_control::ControlOutcome::CredentialStored { .. }
                )
            ) {
                d.wizard_next();
            }
            return Ok(CommandOutput::info(control_notice(ja, &result)));
        }
        Command::Dismiss => {
            d.reject_pending_challenge().await;
            d.cancel_secret_keep_pending();
            None
        }
    };
    Ok(CommandOutput::info(
        outcome.map(|v| outcome_notice(ja, &v)).unwrap_or_default(),
    ))
}
fn local<'a>(ja: bool, japanese: &'a str, english: &'a str) -> &'a str {
    if ja { japanese } else { english }
}

fn intake_notice(ja: bool, refusal: &ChatIntakeRefusal) -> (String, bool) {
    let (japanese, english) = match refusal {
        ChatIntakeRefusal::StaleRound => (
            "前回の状態が古くなっています。最新の状態を確認して、もう一度お送りください。",
            "The previous state is stale. Review the current state and send again.",
        ),
        ChatIntakeRefusal::HeldForTransition => (
            "パートナーの状態が切り替わっています。落ち着いてからもう一度お試しください。",
            "A presence change is in progress; try again once it settles.",
        ),
        ChatIntakeRefusal::NeedsRevalidation { reason } => match reason.0.as_str() {
            "input-over-limit" => (
                "入力が長すぎます。入力内容を短くしてから、もう一度送信してください。",
                "The message is too long to send. Shorten it and send it again.",
            ),
            "stopped-companion" => (
                "パートナーの状態を確認して、実行できる状態に戻してから、もう一度送信してください。",
                "Check the Companion state, restore it to a running state, then send the message again.",
            ),
            "unknown-companion" | "missing-generation-view" => (
                "送信前の接続状態を確認できませんでした。接続を更新してから、もう一度送信してください。",
                "The connection state could not be verified. Refresh the connection, then send the message again.",
            ),
            _ => (
                "送信前の条件を確認できませんでした。現在の状態と設定を確認してから、もう一度送信してください。",
                "The sending conditions could not be verified. Review the current state and settings, then send the message again.",
            ),
        },
    };
    (local(ja, japanese, english).to_string(), true)
}

fn stream_notice(ja: bool, end: ChatStreamEnd) -> (String, bool) {
    match end {
        ChatStreamEnd::Interrupted => (
            local(
                ja,
                "返答は完了前に中断されました。メッセージは既に受け付けられている可能性があるため、会話履歴を更新して現在の状態を確認してください。このメッセージは再送しないでください。",
                "The reply stopped before it was complete. Your message may already have been accepted, so refresh the conversation and check the current state. Do not send this message again.",
            )
            .to_string(),
            true,
        ),
        ChatStreamEnd::Cancelled => (
            local(
                ja,
                "会話が中止されたため、返答を完了できませんでした。メッセージは既に受け付けられているため、会話履歴を確認してください。このメッセージは再送しないでください。",
                "The reply was cancelled after your message was accepted. Review the conversation history. Do not send this message again.",
            )
            .to_string(),
            false,
        ),
        ChatStreamEnd::Stale => (
            local(
                ja,
                "この返答は古い会話の状態に属しており、続行しません。履歴を更新して現在の会話を確認してください。このメッセージは再送しないでください。",
                "This reply belongs to an older conversation state and will not continue. Refresh the history and check the current conversation. Do not send this message again.",
            )
            .to_string(),
            true,
        ),
    }
}

fn chat_error_notice(ja: bool, _error: &DesktopError) -> (String, bool) {
    (
        local(
            ja,
            "チャットの結果が確認できませんでした。メッセージが既に受け付けられている可能性があるため、再送せず接続と履歴を確認してください。",
            "The chat result could not be confirmed. Your message may already have been accepted, so do not resend it; check the connection and conversation history.",
        )
        .to_string(),
        true,
    )
}

fn chat_output(ja: bool, report: ChatSendReport) -> CommandOutput {
    match report {
        ChatSendReport::NoDraft => CommandOutput::info(""),
        ChatSendReport::NotConnected => CommandOutput::error(local(
            ja,
            "Host に接続していません。メッセージは送信されていません。接続が回復したことを確認してから、もう一度試してください。",
            "Not connected to Host. Your message was not sent. Confirm the connection is back, then try again.",
        )),
        ChatSendReport::Refused(refusal) => {
            let (notice, failed) = intake_notice(ja, &refusal);
            CommandOutput {
                notice,
                failed,
                consume_sent_draft: false,
            }
        }
        ChatSendReport::Completed => CommandOutput::consumed_info(""),
        ChatSendReport::StreamEnded(end) => {
            let (notice, failed) = stream_notice(ja, end);
            CommandOutput {
                notice,
                failed,
                consume_sent_draft: true,
            }
        }
        ChatSendReport::ReplyShownHistoryRefreshFailed => CommandOutput {
            notice: local(
                ja,
                "返答は表示されています。ただし、会話履歴を更新できませんでした。「履歴を更新」で確認してください。このメッセージを送信し直さないでください。",
                "The reply is shown, but the conversation history could not be refreshed. Use \"Refresh history\" to check it. Do not send the message again.",
            )
            .to_string(),
            failed: false,
            consume_sent_draft: true,
        },
    }
}

fn result_notice(ja: bool, result: Result<String, DesktopError>) -> (String, bool) {
    match result {
        Ok(notice) => (notice, false),
        Err(DesktopError::BackpressureHold) => (
            ene_desktop::i18n::backpressure_hold(if ja { Locale::Ja } else { Locale::En })
                .to_string(),
            false,
        ),
        Err(DesktopError::Unavailable(reason)) => (reason, true),
        Err(DesktopError::DeniedByBoundary) => (
            ene_desktop::i18n::control_deny(
                if ja { Locale::Ja } else { Locale::En },
                &ene_local_control::FromConfirmation::DeniedByBoundary,
            )
            .to_string(),
            true,
        ),
        Err(_) => (
            local(
                ja,
                "処理を完了できませんでした。接続や現在の状態を確認してください。送信済みの操作は自動再送しません。",
                "The action could not complete. Check the connection and current state. Submitted actions are not automatically retried.",
            )
            .to_string(),
            true,
        ),
    }
}
fn outcome_notice(ja: bool, outcome: &ManagementOutcome) -> String {
    let locale = if ja { Locale::Ja } else { Locale::En };
    ene_desktop::i18n::management_deny(locale, outcome).to_string()
}
fn control_notice(ja: bool, result: &ene_local_control::FromConfirmation) -> String {
    use ene_local_control::{ControlOutcome, DeletionOutcome, FromConfirmation};
    match result {
        FromConfirmation::Outcome(
            ControlOutcome::DeviceApproved { .. } | ControlOutcome::CredentialStored { .. },
        ) => local(ja, "操作を受け付けました。", "The action was accepted."),
        FromConfirmation::Outcome(ControlOutcome::Deletion(DeletionOutcome::Started {
            ..
        })) => local(ja, "削除を開始しました。", "The deletion started."),
        FromConfirmation::Outcome(ControlOutcome::Deletion(
            DeletionOutcome::AlreadyCoveredBy { .. },
        )) => local(
            ja,
            "既存の削除処理に含まれています。",
            "Covered by an existing deletion.",
        ),
        FromConfirmation::Outcome(ControlOutcome::Deletion(DeletionOutcome::HeldByOperation {
            ..
        })) => local(
            ja,
            "別の操作により保留中です。",
            "Held by another operation.",
        ),
        FromConfirmation::Outcome(ControlOutcome::Deletion(
            DeletionOutcome::NeedsClarification,
        )) => local(
            ja,
            "削除対象の確認が必要です。",
            "The deletion target needs clarification.",
        ),
        FromConfirmation::Outcome(ControlOutcome::Deletion(DeletionOutcome::Resumed {
            ..
        })) => local(ja, "再開しました。", "Resumed."),
        FromConfirmation::Outcome(ControlOutcome::Deletion(DeletionOutcome::StaleSweep {
            ..
        })) => local(
            ja,
            "状態が進んだため再開できません。",
            "The state moved on; the resume did not apply.",
        ),
        FromConfirmation::Outcome(ControlOutcome::Deletion(DeletionOutcome::Completed {
            ..
        })) => local(ja, "すでに完了しています。", "Already completed."),
        FromConfirmation::Outcome(ControlOutcome::Deletion(DeletionOutcome::Finalizing {
            ..
        })) => local(ja, "完了処理中です。", "Finalization is in progress."),
        FromConfirmation::Outcome(ControlOutcome::Deletion(DeletionOutcome::Missing)) => {
            local(ja, "対象が見つかりません。", "The target was not found.")
        }
        FromConfirmation::Outcome(ControlOutcome::Rejected { .. })
        | FromConfirmation::Outcome(ControlOutcome::CredentialRefused { .. })
        | FromConfirmation::Outcome(ControlOutcome::CredentialUncommitted { .. })
        | FromConfirmation::Outcome(ControlOutcome::DeviceUnknown { .. })
        | FromConfirmation::DeniedByBoundary
        | FromConfirmation::Unavailable => {
            let locale = if ja { Locale::Ja } else { Locale::En };
            return ene_desktop::i18n::control_deny(locale, result).into();
        }
        _ => local(
            ja,
            "完了を確認できません。現在の状態を確認してください。",
            "Completion could not be confirmed. Review the current state.",
        ),
    }
    .into()
}

fn resume_notice(ja: bool, result: &ene_api::v1::undelivered::ResumeTaskOutcomeWire) -> String {
    use ene_api::v1::undelivered::ResumeTaskOutcomeWire as R;
    match result {
        R::Resumed{..}=>local(ja,"再開しました。","Resumed."),
        R::StalePremise{..}|R::Superseded=>local(ja,"目的や状態が変わっています。作業を更新して選び直してください。","The purpose or revision changed. Refresh and reselect the task."),
        R::TaskTerminal{..}=>local(ja,"この作業は終了しています。","This task has ended."),
        R::AlreadyRunning|R::InFlight=>local(ja,"すでに処理中です。","Already in progress."),
        R::HeldByUnknownEffects=>local(ja,"外部操作の結果が不明なため保留しています。自動再実行しません。","Held because an external action has an unknown result. It will not be replayed automatically."),
        R::ResultAvailable=>local(ja,"結果が届いています。内容を確認してください。","A result is available. Review it before continuing."),
        R::NeedsRevalidation{..}=>local(ja,"再開条件の確認が必要です。","The conditions for resuming need revalidation."),
        R::MissingTask|R::UnknownRef|R::StaleConnection=>local(ja,"選択した作業を確認できません。更新して選び直してください。","The selected task is unavailable. Refresh and select it again."),
        R::RevisionExhausted=>local(ja,"この作業はこれ以上更新できません。","This task cannot accept more revisions."),
        R::Unavailable=>local(ja,"現在、再開できるか確認できません。","The task's availability could not be determined."),
    }.into()
}
fn set_items(current: ModelRc<Item>, next: Vec<Item>, set: impl FnOnce(ModelRc<Item>)) {
    if current.iter().collect::<Vec<_>>() != next {
        set(ModelRc::new(VecModel::from(next)));
    }
}
fn items(rows: &[ene_desktop::ui::presentation::Row]) -> Vec<Item> {
    rows.iter()
        .map(|r| Item {
            key: r.key.as_str().into(),
            title: r.title.as_str().into(),
            body: r.body.as_str().into(),
            meta: r.meta.as_str().into(),
            state: r.state.as_str().into(),
        })
        .collect()
}
fn apply(c: &ChatWindow, m: &ManagementWindow, s: &SurfaceSnapshot, reset_step: bool) {
    c.set_connected(s.connected);
    m.set_connected(s.connected);
    c.set_status(s.status.as_str().into());
    m.set_status(s.status.as_str().into());
    m.set_avatar_available(s.body_available);
    m.set_avatar_visible(s.body_visible);
    let messages: Vec<Message> = s
        .messages
        .iter()
        .map(|r| Message {
            owner: r.owner,
            text: r.text.as_str().into(),
            caption: r.caption.as_str().into(),
        })
        .collect();
    if c.get_messages().iter().collect::<Vec<_>>() != messages {
        c.set_messages(ModelRc::new(VecModel::from(messages)));
    }
    set_items(c.get_tasks(), items(&s.tasks), |v| c.set_tasks(v));
    set_items(c.get_details(), items(&s.details), |v| c.set_details(v));
    c.set_selected_task(s.selected_task.as_str().into());
    c.set_has_task(!s.selected_task.is_empty());
    c.set_progress_summary(if s.tasks.is_empty() {
        "".into()
    } else {
        format!(
            "{} {} · {}",
            s.tasks.len(),
            local(m.get_japanese(), "件の作業", "tasks"),
            local(m.get_japanese(), "詳細を開く", "Open details")
        )
        .into()
    });
    set_items(m.get_memories(), items(&s.memories), |v| m.set_memories(v));
    set_items(m.get_revisions(), items(&s.revisions), |v| {
        m.set_revisions(v)
    });
    set_items(m.get_usage(), items(&s.usage), |v| m.set_usage(v));
    set_items(m.get_caps(), items(&s.caps), |v| m.set_caps(v));
    set_items(m.get_deletions(), items(&s.deletions), |v| {
        m.set_deletions(v)
    });
    m.set_selected_memory(s.selected_memory.as_str().into());
    m.set_selected_deletion(s.selected_deletion.as_str().into());
    m.set_memory_more(s.memory_more);
    m.set_revisions_more(s.revisions_more);
    m.set_usage_more(s.usage_more);
    m.set_can_resume_deletion(s.can_resume_deletion);
    m.set_credential_present(s.credential);
    m.set_consent_assigned(s.consent);
    m.set_assigned_model(s.assigned_model.as_str().into());
    if reset_step {
        m.set_step(s.step);
        m.set_model(s.assigned_model.as_str().into());
    }
    if !m.get_setup_ready() && s.ready {
        show(c);
    }
    m.set_setup_ready(s.ready);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_api::v1::refs::RevalidationReasonWire;
    #[test]
    fn queued_body_copies_are_discarded_and_old_generations_invalidated() {
        let mailbox = Mailbox::default();
        assert!(mailbox.push(Command::Send("private draft".into()), 1));
        assert!(mailbox.push(Command::Secret(Zeroizing::new("private key".into())), 3));
        let epoch = mailbox.epoch.load(Ordering::SeqCst);
        let generation = mailbox.generation.load(Ordering::SeqCst);
        mailbox.erase();
        assert!(mailbox.pop().is_none());
        assert_ne!(epoch, mailbox.epoch.load(Ordering::SeqCst));
        assert_ne!(generation, mailbox.generation.load(Ordering::SeqCst));
    }
    #[test]
    fn erased_inflight_operation_cannot_clear_new_busy_state() {
        let mailbox = Mailbox::default();
        mailbox.push(Command::Send("old".into()), 1);
        let old = mailbox.pop().expect("inflight");
        mailbox.erase();
        mailbox.push(Command::Send("new".into()), 1);
        mailbox.complete(old.lane, old.epoch);
        assert_eq!(mailbox.pending[1].load(Ordering::SeqCst), 1);
    }
    #[test]
    fn communication_backlog_is_bounded() {
        let mailbox = Mailbox::default();
        for _ in 0..16 {
            assert!(mailbox.push(Command::History, 1));
        }
        assert!(!mailbox.push(Command::Send("not accepted".into()), 1));
    }

    #[test]
    fn a_requester_hold_gets_its_own_notice_and_is_not_a_failure() {
        let (ja, ja_failed) = result_notice(true, Err(DesktopError::BackpressureHold));
        let (en, en_failed) = result_notice(false, Err(DesktopError::BackpressureHold));
        assert_eq!(ja, ene_desktop::i18n::backpressure_hold(Locale::Ja));
        assert_eq!(en, ene_desktop::i18n::backpressure_hold(Locale::En));
        assert!(!ja_failed && !en_failed, "a hold is not an error notice");
        assert!(!ja.contains("自動再送"));
        assert!(!en.contains("could not complete"));
    }

    #[test]
    fn other_failures_keep_the_generic_no_resend_notice() {
        let (en, english_failed) =
            result_notice(false, Err(DesktopError::Transport(String::from("x"))));
        assert!(english_failed);
        assert!(en.contains("not automatically retried"));
        let (ja, japanese_failed) =
            result_notice(true, Err(DesktopError::Control(String::from("x"))));
        assert!(japanese_failed);
        assert!(ja.contains("自動再送"));
    }

    #[test]
    fn a_hold_is_not_resubmitted_by_the_launcher() {
        assert!(!launcher_retries(&DesktopError::BackpressureHold));
        assert!(launcher_retries(&DesktopError::Transport(String::from(
            "x"
        ))));
    }

    #[test]
    fn successful_results_keep_their_notice() {
        let (notice, failed) = result_notice(false, Ok(String::from("accepted")));
        assert_eq!(notice, "accepted");
        assert!(!failed);
    }

    #[test]
    fn chat_reports_are_localized_without_exposing_wire_reasons() {
        let reports = [
            ChatSendReport::NotConnected,
            ChatSendReport::Refused(ChatIntakeRefusal::StaleRound),
            ChatSendReport::Refused(ChatIntakeRefusal::HeldForTransition),
            ChatSendReport::Refused(ChatIntakeRefusal::NeedsRevalidation {
                reason: RevalidationReasonWire(String::from("input-over-limit")),
            }),
            ChatSendReport::StreamEnded(ChatStreamEnd::Interrupted),
            ChatSendReport::StreamEnded(ChatStreamEnd::Cancelled),
            ChatSendReport::StreamEnded(ChatStreamEnd::Stale),
            ChatSendReport::ReplyShownHistoryRefreshFailed,
        ];
        for report in reports {
            let ja = chat_output(true, report.clone());
            let en = chat_output(false, report);
            assert_ne!(ja.notice, en.notice);
            assert!(!ja.notice.contains("input-over-limit"));
            assert!(!en.notice.contains("input-over-limit"));
            assert!(!ja.notice.contains("client is not connected"));
            assert!(!en.notice.contains("client is not connected"));
        }
    }

    #[test]
    fn a_technical_chat_failure_does_not_advise_an_immediate_resend() {
        let error = DesktopError::Transport(String::from("provider response lost"));
        let (ja, ja_failed) = chat_error_notice(true, &error);
        let (en, en_failed) = chat_error_notice(false, &error);
        assert!(ja_failed && en_failed);
        assert!(ja.contains("再送せず"));
        assert!(en.contains("do not resend"));
        assert!(!ja.contains("provider response lost"));
        assert!(!en.contains("provider response lost"));
    }

    #[test]
    fn a_history_refresh_failure_is_a_nonfatal_no_resend_notice() {
        let output = chat_output(true, ChatSendReport::ReplyShownHistoryRefreshFailed);
        assert!(!output.failed);
        assert!(output.consume_sent_draft);
        assert!(output.notice.contains("表示されています"));
        assert!(output.notice.contains("送信し直さないでください"));
        let english = chat_output(false, ChatSendReport::ReplyShownHistoryRefreshFailed);
        assert!(english.notice.contains("reply is shown"));
        assert!(english.notice.contains("Do not send the message again"));
    }

    #[test]
    fn chat_draft_disposition_distinguishes_pre_submit_and_consumed_outcomes() {
        assert!(!chat_output(true, ChatSendReport::NotConnected).consume_sent_draft);
        assert!(
            !chat_output(true, ChatSendReport::Refused(ChatIntakeRefusal::StaleRound),)
                .consume_sent_draft
        );
        assert!(chat_output(true, ChatSendReport::Completed).consume_sent_draft);
        assert!(
            chat_output(
                true,
                ChatSendReport::StreamEnded(ChatStreamEnd::Interrupted),
            )
            .consume_sent_draft
        );
    }

    #[test]
    fn cancelled_stream_is_not_styled_as_a_failure() {
        let output = chat_output(false, ChatSendReport::StreamEnded(ChatStreamEnd::Cancelled));
        assert!(!output.failed);
    }

    #[test]
    fn completed_redraw_drains_pending_interaction_with_paint_timestamp() {
        let pending = Mutex::new(VecDeque::from([PendingPaint {
            start: InteractionStart {
                operation: "cancel_task",
                input_monotonic_ns: 10,
            },
            host_outcome_monotonic_ns: 20,
        }]));
        let samples = drain_painted_interactions(&pending, 30);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].operation, "cancel_task");
        assert_eq!(samples[0].input_monotonic_ns, 10);
        assert_eq!(samples[0].host_outcome_monotonic_ns, 20);
        assert_eq!(samples[0].gui_painted_monotonic_ns, 30);
        assert!(pending.lock().expect("pending paint lock").is_empty());
    }
}
