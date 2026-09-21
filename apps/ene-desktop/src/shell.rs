use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};
use std::time::Duration;

use ene_api::v1::management::ManagementOutcome;
use ene_config::{Config, resolve_data_dir};
use ene_desktop::i18n::Locale;
use ene_desktop::measure::{InteractionSample, InteractionTraceLine, monotonic_ns};
use ene_desktop::ui::presentation::{SurfaceSnapshot, parse_cap};
use ene_desktop::ui::{DesktopError, DesktopRuntime};
use ene_desktop_ui::{ChatWindow, Item, ManagementWindow, Message};
use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, Model, ModelRc, RenderingState, VecModel};
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
            Self::CancelTask(_) => Some("cancel_task"),
            _ => None,
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
    host_intake_monotonic_ns: u64,
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
        if busy {
            return false;
        }
        if !self.mailbox.push(command, lane) {
            management.set_notice(
                local(
                    management.get_japanese(),
                    "操作が混み合っています。少し待ってください。",
                    "Too many pending actions. Please wait.",
                )
                .into(),
            );
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
            m.invoke_clear_secret();
            ene_desktop_ui::discard_secret_input(&m);
            m.set_can_confirm(false);
            m.set_confirmation_key("".into());
            m.set_confirmation_target("".into());
            m.set_confirmation_title("".into());
            m.set_confirmation_description("".into());
            if m.get_page() == 7 {
                m.set_page(6);
            }
        }
        self.mailbox.push(Command::Dismiss, 0);
    }
}

pub fn run() -> Result<(), DesktopError> {
    let config = Config::load(None).map_err(|e| DesktopError::Protocol(e.to_string()))?;
    let data_dir = resolve_data_dir(&config)
        .ok_or_else(|| DesktopError::HostLaunch("no data directory resolved".into()))?;
    // Two roles, one binary. The Host sets the marker while spawning the
    // process it hands the private confirmation channel to; without it this
    // process is the user-started launcher, which opens the Host's GUI and
    // exits. A user or requester cannot aim this switch: it is an environment
    // value only the Host writes.
    match std::env::var(ene_local_control::CONFIRMATION_MODE_ENV) {
        Ok(value) if value == ene_local_control::CONFIRMATION_MODE_STDIO => run_gui(data_dir),
        _ => run_launcher(data_dir),
    }
}

/// The short-lived launcher: ask the serving Host to open its GUI, then exit.
///
/// The launcher never holds a seat: it only requests. If no Host is serving it
/// starts one first, which is what keeps a manual `ene-core serve` out of the
/// normal setup path.
fn run_launcher(data_dir: std::path::PathBuf) -> Result<(), DesktopError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| DesktopError::Transport(e.to_string()))?;
    runtime.block_on(async move {
        if !ene_desktop::host_launch::host_is_serving(&data_dir) {
            let binary = ene_desktop::host_launch::locate_host_binary().ok_or_else(|| {
                DesktopError::HostLaunch(String::from("ene-core binary was not found"))
            })?;
            let detached = ene_desktop::host_launch::detach_serve(&data_dir, &binary)
                .map_err(|error| DesktopError::HostLaunch(error.to_string()))?;
            if detached.pid == 0 {
                return Err(DesktopError::HostLaunch(String::from(
                    "detached host reported pid 0",
                )));
            }
        }
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
                    attempts = attempts.saturating_add(1);
                    if attempts >= 80 {
                        return Err(error);
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    })
}

/// The Host-spawned GUI: adopt the inherited confirmation channel and run the
/// windows.
fn run_gui(data_dir: std::path::PathBuf) -> Result<(), DesktopError> {
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
    let installed = chat
        .window()
        .set_rendering_notifier(move |state, _graphics| {
            if !matches!(state, RenderingState::AfterRendering) {
                return;
            }
            let paints = {
                let mut pending = pending
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                pending.drain(..).collect::<Vec<_>>()
            };
            let painted = monotonic_ns();
            for paint in paints {
                let sample = InteractionSample {
                    operation: paint.start.operation.to_string(),
                    input_monotonic_ns: paint.start.input_monotonic_ns,
                    host_intake_monotonic_ns: paint.host_intake_monotonic_ns,
                    gui_painted_monotonic_ns: painted,
                };
                append_interaction_trace(&trace_path, sample);
            }
        });
    match installed {
        Ok(()) => true,
        Err(error) => {
            eprintln!("interaction paint evidence unavailable: {error}");
            false
        }
    }
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
fn show<C: ComponentHandle>(window: &C) {
    match window.show() {
        Ok(_) | Err(_) => {}
    }
    window.window().set_minimized(false);
    window.window().with_winit_window(|w| w.focus_window());
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
            if s.submit(Command::Send(text.to_string()), 1)
                && let Some(c) = s.chat.upgrade()
            {
                c.set_draft("".into());
            }
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
    // The opaque confirmation key never appears as a visible label.
    m.on_confirm_owner({
        let s = s.clone();
        move || {
            if let Some(m) = s.management.upgrade()
                && m.get_can_confirm()
            {
                s.submit(Command::Confirm(m.get_confirmation_key().to_string()), 3);
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
                    c.invoke_clear_copies();
                    m.invoke_clear_copies();
                    c.set_busy(false);
                    c.set_task_busy(false);
                    m.set_busy(false);
                    m.set_confirmation_key("".into());
                    ene_desktop_ui::erase_surface_copies(&c, &m)
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
        let notified = s.mailbox.wake.notified();
        let Some(request) = s.mailbox.pop() else {
            tokio::select! { ()=notified => {}, ()=tokio::time::sleep(Duration::from_millis(250)) => desktop.tick() }
            continue;
        };
        if request.epoch != s.mailbox.epoch.load(Ordering::SeqCst) {
            continue;
        }
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
        let result = execute(&mut desktop, request.command).await;
        // The Host outcome is a conservative upper bound on intake: the Host
        // necessarily accepted or refused the operation before this point.
        let measured = if s.interaction_paint_evidence && result.is_ok() {
            request.interaction.map(|start| PendingPaint {
                start,
                host_intake_monotonic_ns: monotonic_ns(),
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
            if let (Some(c),Some(m))=(surfaces.chat.upgrade(),surfaces.management.upgrade()) {
                apply(&c,&m,&snap,reset_step);
                c.set_busy(surfaces.mailbox.pending[1].load(Ordering::SeqCst)>0);
                c.set_task_busy(surfaces.mailbox.pending[2].load(Ordering::SeqCst)>0);
                m.set_busy(surfaces.mailbox.pending[3].load(Ordering::SeqCst)>0);
                let ja=m.get_japanese();
                let failed=result.is_err();
                c.set_notice_error(failed); m.set_notice_error(failed);
                let notice=result.unwrap_or_else(|_| local(ja, "処理を完了できませんでした。接続や現在の状態を確認してください。送信済みの操作は自動再送しません。", "The action could not complete. Check the connection and current state. Submitted actions are not automatically retried.").into());
                match request.lane { 1 => { c.set_notice(notice.into()); }, 2 => { c.set_task_notice(notice.into()); }, _ => { if request.management_page==Some(m.get_page()) { m.set_notice(notice.into()); } } }
                if let Some(measured) = measured {
                    surfaces
                        .pending_paints
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push_back(measured);
                    c.window().request_redraw();
                }
                if request.generation == surfaces.mailbox.generation.load(Ordering::SeqCst) {
                    if let Some(confirm)=&snap.confirmation {
                        if confirmation_action {
                            m.set_confirmation_key(confirm.key.as_str().into()); m.set_confirmation_title(confirm.title.as_str().into());
                            m.set_confirmation_description(confirm.description.as_str().into()); m.set_confirmation_target(confirm.target.as_str().into());
                            m.set_can_confirm(true); m.set_page(7); show(&m);
                        }
                    } else if m.get_page()==7 { m.set_page(6); m.set_can_confirm(false); m.set_confirmation_key("".into()); }
                }
            }
            match tx.send(()) { Ok(_) | Err(_) => {} }
        }).is_err() { return; }
        if rx.await.is_err() {
            return;
        }
    }
}
async fn execute(d: &mut DesktopRuntime, command: Command) -> Result<String, DesktopError> {
    let ja = d.surface_snapshot().japanese;
    let outcome = match command {
        Command::Startup => {
            d.ensure_host(None)?;
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
            d.send_text().await?;
            None
        }
        Command::Workspace(path) => Some(
            d.select_workspace_folder(std::path::Path::new(&path))
                .await?,
        ),
        Command::Resume(key, text) => {
            d.check_task_key(&key)?;
            let result = d.resume_displayed_task(text).await?;
            return Ok(resume_notice(ja, &result));
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
            d.refresh_deletion_requests().await?;
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
            return Ok(control_notice(ja, &result));
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
            return Ok(control_notice(ja, &result));
        }
        Command::Dismiss => {
            d.cancel_secret();
            None
        }
    };
    Ok(outcome.map(|v| outcome_notice(ja, &v)).unwrap_or_default())
}
fn local<'a>(ja: bool, japanese: &'a str, english: &'a str) -> &'a str {
    if ja { japanese } else { english }
}
fn outcome_notice(ja: bool, outcome: &ManagementOutcome) -> String {
    match outcome {
        ManagementOutcome::AppliedAsOneTime | ManagementOutcome::StoredAsRuleView { .. } => {
            local(ja, "反映しました。", "Applied.")
        }
        ManagementOutcome::NeedsClarification => local(
            ja,
            "対象や内容を具体的にしてください。",
            "Clarify the target or requested change.",
        ),
        ManagementOutcome::DeniedByBoundary => local(
            ja,
            "この操作は許可されませんでした。",
            "This action was denied.",
        ),
        ManagementOutcome::StaleBaseView { .. } => local(
            ja,
            "表示後に状態が変わりました。更新して確認してください。",
            "The state changed. Refresh and review before acting.",
        ),
        ManagementOutcome::HeldByOperation => local(
            ja,
            "要求を保留しています。状態を確認してください。",
            "The request is on hold. Review its status.",
        ),
    }
    .into()
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
    if reset_step {
        m.set_step(s.step);
    }
    if !m.get_setup_ready() && s.ready {
        show(c);
    }
    m.set_setup_ready(s.ready);
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
