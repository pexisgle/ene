#![cfg(any(unix, windows))]

use std::collections::VecDeque;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome,
    RationaleOrigin, consent_target,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{BaseViewMark, CommandWireId};
use ene_core::conn;
use ene_core::host_control;
use ene_core::serve::{CoreError, CredStore, HostHandle};
use ene_credential::MemoryVersionedStore;
use ene_desktop::i18n::{self, Label, Locale};
use ene_desktop::session;
use ene_desktop::ui::{DesktopRuntime, Page};
use ene_inference::{ProviderRequest, ProviderResponse, ProviderTransport};
use ene_local_control::{ControlOutcome, FromConfirmation};

const MODEL: &str = "gpt-slice-test";
const SECRET: &str = "sk-stage7-c1-secret-7719";

struct LearningTransport {
    replies: Mutex<VecDeque<String>>,
    learning: Mutex<VecDeque<String>>,
    inputs: Mutex<Vec<String>>,
    sends: AtomicUsize,
    learning_calls: Arc<tokio::sync::Semaphore>,
    park: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    learning_park: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

impl LearningTransport {
    fn with_replies(replies: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.iter().map(|text| (*text).to_string()).collect()),
            learning: Mutex::new(VecDeque::new()),
            inputs: Mutex::new(Vec::new()),
            sends: AtomicUsize::new(0),
            learning_calls: Arc::new(tokio::sync::Semaphore::new(0)),
            park: Mutex::new(None),
            learning_park: Mutex::new(None),
        })
    }

    fn push_learning(&self, answer: &str) {
        self.learning
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push_back(answer.to_owned());
    }

    #[expect(clippy::expect_used, reason = "test fixture helper")]
    async fn wait_learning(&self) {
        let permit = self
            .learning_calls
            .acquire()
            .await
            .expect("the learning-call semaphore stays open");
        permit.forget();
    }

    fn park_learning(&self) -> tokio::sync::oneshot::Sender<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        *self
            .learning_park
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(rx);
        tx
    }

    fn all_inputs(&self) -> Vec<String> {
        self.inputs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn last_dialogue_input(&self) -> Option<String> {
        self.all_inputs()
            .into_iter()
            .rfind(|input| !input.contains("learning formation pass"))
    }
}

impl ProviderTransport for LearningTransport {
    fn complete(
        &self,
        req: ProviderRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<ProviderResponse, ene_inference::InferenceTechnicalError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            self.inputs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(req.input.clone());
            let learning = req.input.contains("learning formation pass");
            if learning {
                let parked = self
                    .learning_park
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take();
                if let Some(rx) = parked {
                    match rx.await {
                        Ok(()) | Err(_) => {}
                    }
                }
                let answer = self
                    .learning
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .pop_front()
                    .unwrap_or_else(|| {
                        String::from(r#"{"summary": "Nothing worth keeping.", "memories": []}"#)
                    });
                self.learning_calls.add_permits(1);
                return Ok(ProviderResponse {
                    text: answer,
                    usage: None,
                });
            }
            self.sends.fetch_add(1, Ordering::SeqCst);
            let parked = self
                .park
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            if let Some(rx) = parked {
                match rx.await {
                    Ok(()) | Err(_) => {}
                }
            }
            let reply = self
                .replies
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop_front()
                .unwrap_or_else(|| String::from("ok"));
            Ok(ProviderResponse {
                text: reply,
                usage: None,
            })
        })
    }
}

struct ServingTask {
    shutdown: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<Result<(), CoreError>>,
}

impl ServingTask {
    fn start(dir: &Path, handle: Arc<HostHandle>, transport: Arc<LearningTransport>) -> Self {
        let (shutdown, rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(conn::run_until_shutdown(
            dir.to_path_buf(),
            handle,
            transport,
            rx,
        ));
        Self { shutdown, task }
    }

    #[expect(clippy::expect_used, reason = "test fixture helper")]
    async fn shutdown_and_join(self) {
        self.shutdown.send_replace(true);
        tokio::time::timeout(Duration::from_secs(30), self.task)
            .await
            .expect("serving shutdown must drain")
            .expect("serving task must join")
            .expect("serving shutdown must succeed");
    }
}

#[expect(clippy::panic, reason = "test fixture helper")]
async fn open_host(dir: &Path) -> Arc<HostHandle> {
    match HostHandle::open_with_cred_store(
        dir,
        CredStore::MemoryVersioned(MemoryVersionedStore::new()),
    )
    .await
    {
        Ok(handle) => {
            handle.set_client_erasure_wait_for_tests(Duration::from_millis(200));
            Arc::new(handle)
        }
        Err(error) => panic!("host must open: {error}"),
    }
}

async fn wait_for_control(dir: &Path) -> bool {
    for _ in 0..200 {
        #[cfg(unix)]
        if tokio::net::UnixStream::connect(host_control::control_socket_path(dir))
            .await
            .is_ok()
        {
            return true;
        }
        #[cfg(windows)]
        if host_control::ControlClient::connect(dir).await.is_ok() {
            tokio::task::yield_now().await;
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[expect(clippy::expect_used, clippy::panic, reason = "test fixture helper")]
async fn pair_and_seat(desktop: &mut DesktopRuntime, handle: &Arc<HostHandle>) {
    let channel = host_control::seat_test_gui_for_tests(handle).expect("private channel");
    desktop
        .attach_confirmation(channel)
        .expect("the private channel is the seat");
    desktop
        .connect_or_begin_pairing()
        .await
        .expect("pairing must challenge");
    match desktop.confirm_owner().await.expect("owner confirm pairs") {
        FromConfirmation::Outcome(ControlOutcome::DeviceApproved { .. }) => {}
        other => panic!("expected DeviceApproved, got {other:?}"),
    }
}

#[expect(clippy::expect_used, clippy::panic, reason = "test fixture helper")]
async fn complete_setup(desktop: &mut DesktopRuntime) {
    desktop.set_secret(String::from(SECRET));
    desktop
        .begin_credential_put()
        .await
        .expect("credential put must challenge");
    match desktop
        .confirm_owner()
        .await
        .expect("owner confirm stores the key")
    {
        FromConfirmation::Outcome(ControlOutcome::CredentialStored { .. }) => {}
        other => panic!("expected CredentialStored, got {other:?}"),
    }
    desktop.set_model(String::from(MODEL));
    let assigned = desktop.assign_model().await.expect("dialogue assign");
    assert!(
        matches!(assigned, ManagementOutcome::StoredAsRuleView { .. }),
        "dialogue assignment must store, got {assigned:?}"
    );
    assign_learning(desktop).await;
}

#[expect(clippy::expect_used, clippy::panic, reason = "test fixture helper")]
async fn assign_learning(desktop: &mut DesktopRuntime) {
    let mut client = desktop.take_client().expect("paired client");
    let view = session::fetch_setup_view(&mut client)
        .await
        .expect("setup view for learning assignment");
    let intent = ManagementIntent {
        intent_id: CommandWireId(uuid::Uuid::new_v4()),
        kind: ManagementIntentKind::ManageRuleConsentCap,
        target: consent_target(
            "learning",
            session::SETUP_PROVIDER_OPENAI,
            MODEL,
            &format!(
                "{}:{}",
                session::SETUP_PROVIDER_OPENAI,
                session::SETUP_CREDENTIAL_LABEL
            ),
        ),
        base_view: BaseViewMark(view.mark.0),
        rationale: IntentRationaleWire {
            origin: RationaleOrigin::ManagementSurface,
            quote: None,
        },
        confirmed: false,
    };
    let answer = tokio::time::timeout(
        Duration::from_secs(15),
        client.request(WirePayload::ManagementIntent(intent)),
    )
    .await
    .expect("learning assignment must not time out")
    .expect("learning assignment must answer");
    desktop.restore_client(client);
    match answer {
        WirePayload::ManagementOutcome(ManagementOutcome::StoredAsRuleView { .. }) => {}
        other => panic!("learning assignment must store, got {other:?}"),
    }
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
async fn say(desktop: &mut DesktopRuntime, text: &str) {
    desktop.composer_mut().set_draft(text.to_owned());
    desktop.send_text().await.expect("chat turn must complete");
}

#[expect(clippy::expect_used, clippy::panic, reason = "test fixture helper")]
async fn wait_for_memory(desktop: &mut DesktopRuntime, expected: &str) {
    for _ in 0..100 {
        desktop.refresh_memory().await.expect("memory list");
        if desktop
            .memory()
            .rows()
            .iter()
            .any(|row| row.content.contains(expected) || row.recall.contains(expected))
            || desktop.snapshot().memory_panel.contains(expected)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    desktop.refresh_memory().await.expect("final memory list");
    panic!(
        "the memory GUI never showed {expected:?}: {}",
        desktop.snapshot().memory_panel
    );
}

#[expect(clippy::expect_used, clippy::panic, reason = "test fixture helper")]
async fn wait_for_revisions(desktop: &mut DesktopRuntime, expected: &str) {
    for _ in 0..100 {
        desktop.refresh_memory().await.expect("memory list");
        let ids: Vec<String> = desktop
            .memory()
            .rows()
            .iter()
            .map(|row| row.id.clone())
            .collect();
        for id in ids {
            desktop
                .open_memory_revisions(&id)
                .await
                .expect("revision page");
            if desktop.snapshot().memory_panel.contains(expected)
                || desktop.memory().revisions().iter().any(|revision| {
                    revision.change.contains(expected)
                        || revision.content.contains(expected)
                        || revision
                            .grounds
                            .as_deref()
                            .is_some_and(|grounds| grounds.contains(expected))
                })
            {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "the revision GUI never showed {expected:?}: {}",
        desktop.snapshot().memory_panel
    );
}

fn recall_section(input: &str) -> String {
    let Some(start) = input.find("Relevant memories:") else {
        return String::new();
    };
    let rest = &input[start..];
    let end = rest.find("Recent conversation:").unwrap_or(rest.len());
    rest[..end].to_owned()
}

#[tokio::test]
async fn memory_gui_confirms_acceptance_3_1_to_3_10() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport = LearningTransport::with_replies(&["ok"]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_seat(&mut desktop, &handle).await;
    complete_setup(&mut desktop).await;
    desktop.try_spawn_body(&desktop.bundled_sample_asset());
    assert_eq!(desktop.snapshot().body_status, "Absent");

    desktop.open_page(Page::Memory);
    desktop.refresh_memory().await.expect("empty memory page");
    assert!(
        desktop.memory().rows().is_empty(),
        "the GUI must not invent memories: {:?}",
        desktop.memory().rows()
    );
    assert!(
        desktop
            .memory()
            .last_request()
            .is_some_and(|request| request.memory_after.is_none()
                && request.memory_revisions_of.is_none()
                && request.sections == [String::from("memory")]),
        "the first list page is a Host memory section with no cursor"
    );

    transport.push_learning(
        r#"{"summary": "The owner prefers jasmine tea in the morning.", "memories": [{"action": "create", "content": "The owner prefers jasmine tea in the morning.", "importance": 5, "temporal": "enduring"}]}"#,
    );
    say(
        &mut desktop,
        "これ覚えておいて! remember that I prefer jasmine tea in the morning",
    )
    .await;
    transport.wait_learning().await;
    wait_for_memory(
        &mut desktop,
        "The owner prefers jasmine tea in the morning.",
    )
    .await;
    desktop.open_page(Page::Memory);
    let snap = desktop.snapshot();
    assert_eq!(snap.page, "Memory");
    assert_eq!(snap.memories.len(), 1, "one memory, not one per message");
    let row = &snap.memories[0];
    assert_eq!(row.scope, "companion");
    assert_eq!(row.importance, "5");
    assert!(
        row.created_at.contains('T'),
        "created-at is the Host timestamp: {}",
        row.created_at
    );
    assert!(
        snap.memory_panel.contains("created-at=") && snap.memory_panel.contains("scope=companion"),
        "the panel projects Host facts: {}",
        snap.memory_panel
    );
    assert!(
        !snap.memory_panel.contains("grounds summary"),
        "the list page does not inflate revisions: {}",
        snap.memory_panel
    );
    let tea_id = row.id.clone();
    desktop
        .open_memory_revisions(&tea_id)
        .await
        .expect("revision page");
    let snap = desktop.snapshot();
    assert!(
        snap.memory_revisions
            .iter()
            .any(|revision| revision.change == "initial"
                && revision
                    .grounds
                    .as_deref()
                    .is_some_and(|grounds| grounds.contains("jasmine tea"))),
        "Experience Summary grounds: {:?}",
        snap.memory_revisions
    );
    assert!(
        desktop
            .memory()
            .last_request()
            .is_some_and(
                |request| request.memory_revisions_of.as_deref() == Some(tea_id.as_str())
                    && request.memory_after.is_none()
            ),
        "revisions use memory_revisions_of, not a list scan"
    );

    transport.push_learning(r#"{"summary": "Small talk about the weather.", "memories": []}"#);
    say(&mut desktop, "nice weather today").await;
    transport.wait_learning().await;
    desktop.refresh_memory().await.expect("after chatter");
    assert_eq!(desktop.memory().rows().len(), 1, "chatter stores nothing");

    transport.push_learning(
        r#"{"summary": "The owner mentioned tea again.", "memories": [{"action": "update", "target": 1, "change": "reinforced", "content": "The owner prefers jasmine tea in the morning."}]}"#,
    );
    say(&mut desktop, "I still love jasmine tea").await;
    transport.wait_learning().await;
    wait_for_revisions(&mut desktop, "reinforced").await;
    desktop.refresh_memory().await.expect("after reinforce");
    assert_eq!(desktop.memory().rows().len(), 1, "no duplicate memory");

    transport.push_learning(
        r#"{"summary": "The owner corrected the earlier memory.", "memories": [{"action": "update", "target": 1, "change": "corrected_initially_wrong", "content": "The owner never liked jasmine tea."}]}"#,
    );
    say(&mut desktop, "actually I never liked jasmine tea").await;
    transport.wait_learning().await;
    transport.push_learning(
        r#"{"summary": "The preference changed over time.", "memories": [{"action": "update", "target": 1, "change": "changed_since", "content": "The owner prefers coffee now."}]}"#,
    );
    say(&mut desktop, "I moved on to coffee last month").await;
    transport.wait_learning().await;
    wait_for_revisions(&mut desktop, "changed-since").await;
    desktop
        .open_memory_revisions(&tea_id)
        .await
        .expect("history after change");
    let changes: Vec<&str> = desktop
        .memory()
        .revisions()
        .iter()
        .map(|revision| revision.change.as_str())
        .collect();
    assert!(
        changes.contains(&"initial")
            && changes.contains(&"corrected-initially-wrong")
            && changes.contains(&"changed-since"),
        "past revisions remain: {changes:?}"
    );
    desktop
        .refresh_memory()
        .await
        .expect("current after change");
    assert!(
        desktop
            .memory()
            .rows()
            .iter()
            .any(|row| row.content.contains("coffee")),
        "current recognition is coffee: {:?}",
        desktop.memory().rows()
    );

    transport.push_learning(&format!(
        r#"{{"summary": "The owner shared a key: {SECRET}.", "memories": [{{"action": "create", "content": "The owner's key is {SECRET}.", "importance": 5, "temporal": "enduring"}}]}}"#
    ));
    say(&mut desktop, &format!("remember my key {SECRET}")).await;
    transport.wait_learning().await;
    wait_for_memory(&mut desktop, "[credential]").await;
    desktop
        .refresh_history()
        .await
        .expect("history after secret");
    let redacted_id = desktop
        .memory()
        .rows()
        .iter()
        .find(|row| row.content.contains("[credential]"))
        .expect("redacted memory")
        .id
        .clone();
    desktop
        .open_memory_revisions(&redacted_id)
        .await
        .expect("redacted revisions");
    let snap = desktop.snapshot();
    assert!(
        snap.memories
            .iter()
            .all(|row| !row.content.contains(SECRET))
            && snap.memory_revisions.iter().all(|revision| {
                !revision.content.contains(SECRET)
                    && revision
                        .grounds
                        .as_deref()
                        .is_none_or(|grounds| !grounds.contains(SECRET))
            })
            && !snap.memory_panel.contains(SECRET)
            && snap.history.iter().all(|line| !line.contains(SECRET)),
        "Host Memory and History must not keep the registered secret: {}",
        snap.memory_panel
    );
    let memory_debug = format!("{:?}", desktop.memory());
    assert!(
        !memory_debug.contains(SECRET),
        "Memory page Debug must not leak: {memory_debug}"
    );
    for (position, input) in transport.all_inputs().iter().enumerate() {
        assert!(
            !input.contains(SECRET),
            "provider input {position} must not carry the credential"
        );
    }
    assert!(
        !snap.memory_panel.contains(SECRET),
        "the memory panel must not show the secret: {}",
        snap.memory_panel
    );

    server.shutdown_and_join().await;
    handle
        .run_startup_mutations()
        .await
        .expect("restart mutations");
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    desktop
        .reconnect()
        .await
        .expect("reconnect after host restart");
    desktop
        .refresh_memory()
        .await
        .expect("memory after restart");
    assert!(
        desktop
            .memory()
            .rows()
            .iter()
            .any(|row| row.content.contains("coffee")),
        "restart keeps current recognition: {:?}",
        desktop.memory().rows()
    );
    desktop
        .open_memory_revisions(&tea_id)
        .await
        .expect("revisions after restart");
    assert!(
        desktop
            .memory()
            .revisions()
            .iter()
            .any(|revision| revision.change == "initial"),
        "restart keeps revision history"
    );
    transport.push_learning(r#"{"summary": "Nothing new.", "memories": []}"#);
    say(&mut desktop, "what do I drink now?").await;
    transport.wait_learning().await;
    let recalled = recall_section(
        &transport
            .last_dialogue_input()
            .expect("the dialogue input is recorded"),
    );
    assert!(
        recalled.contains("coffee"),
        "the current recognition is recalled after restart: {recalled}"
    );

    transport.push_learning(
        r#"{"summary": "The owner asked to let the drink topic rest.", "memories": [{"action": "forget", "target": 2}]}"#,
    );
    say(
        &mut desktop,
        "今の話は忘れて. forget about my drink preference",
    )
    .await;
    transport.wait_learning().await;
    wait_for_memory(&mut desktop, "suppressed").await;
    assert!(
        desktop
            .memory()
            .rows()
            .iter()
            .any(|row| row.content.contains("coffee") && row.recall == "suppressed"),
        "forget is deprioritize, not wipe: {:?}",
        desktop.memory().rows()
    );
    wait_for_revisions(&mut desktop, "forgotten").await;
    transport.push_learning(r#"{"summary": "Nothing new.", "memories": []}"#);
    say(&mut desktop, "what do I drink now?").await;
    transport.wait_learning().await;
    let recalled = recall_section(
        &transport
            .last_dialogue_input()
            .expect("the dialogue input is recorded"),
    );
    assert!(
        !recalled.contains("coffee"),
        "suppressed memory is not recalled: {recalled}"
    );

    desktop.set_locale(Locale::En);
    desktop.refresh_memory().await.expect("after locale");
    assert!(
        desktop
            .memory()
            .rows()
            .iter()
            .any(|row| row.content.contains("coffee")),
        "JA/EN must not rewrite memory content"
    );
    assert_ne!(
        i18n::label(Locale::Ja, Label::Memory),
        i18n::label(Locale::En, Label::Memory)
    );

    server.shutdown_and_join().await;
}

#[tokio::test]
async fn memory_gui_pages_at_the_host_instead_of_scanning() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport = LearningTransport::with_replies(&["ok"]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_seat(&mut desktop, &handle).await;
    complete_setup(&mut desktop).await;

    for batch in 0..5 {
        let creates: Vec<String> = (0..5)
            .map(|index| {
                format!(
                    "{{\"action\": \"create\", \"content\": \"batch {batch} memory {index}\", \"importance\": 3, \"temporal\": \"enduring\"}}"
                )
            })
            .collect();
        transport.push_learning(&format!(
            "{{\"summary\": \"Batch {batch} of durable memories.\", \"memories\": [{}]}}",
            creates.join(", ")
        ));
        say(&mut desktop, &format!("remember batch {batch}")).await;
        transport.wait_learning().await;
    }
    wait_for_memory(&mut desktop, "batch 4 memory 4").await;

    desktop.open_page(Page::Memory);
    desktop.refresh_memory().await.expect("first Host page");
    assert_eq!(
        desktop.memory().rows().len(),
        20,
        "the GUI shows one Host page, not a truncated full scan"
    );
    let cursor = desktop
        .memory()
        .next_after()
        .expect("older memories remain")
        .to_owned();
    assert!(
        desktop
            .memory()
            .last_request()
            .is_some_and(|request| request.memory_after.is_none()),
        "the first page starts at the newest"
    );
    desktop.page_older_memories().await.expect("next Host page");
    assert!(
        desktop
            .memory()
            .last_request()
            .is_some_and(
                |request| request.memory_after.as_deref() == Some(cursor.as_str())
                    && request.memory_revisions_of.is_none()
            ),
        "the next page rides memory_after"
    );
    assert_eq!(
        desktop.memory().rows().len(),
        25,
        "the second Host page is appended after the caller asked for it"
    );
    assert!(
        desktop
            .memory()
            .rows()
            .iter()
            .any(|row| row.content == "batch 0 memory 0"),
        "the oldest memory is reachable: {:?}",
        desktop.memory().rows()
    );
    let oldest = desktop
        .memory()
        .rows()
        .iter()
        .find(|row| row.content == "batch 0 memory 0")
        .expect("oldest row")
        .id
        .clone();
    desktop
        .open_memory_revisions(&oldest)
        .await
        .expect("oldest revisions");
    assert!(
        desktop
            .memory()
            .revisions()
            .iter()
            .any(|revision| revision.change == "initial" && revision.grounds.as_ref().is_some()),
        "grounds stay on the revision page: {:?}",
        desktop.memory().revisions()
    );

    server.shutdown_and_join().await;
}

#[tokio::test]
async fn learning_barrier_gui_does_not_invent_formation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport = LearningTransport::with_replies(&["ok"]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_seat(&mut desktop, &handle).await;
    complete_setup(&mut desktop).await;

    transport.push_learning(
        r#"{"summary": "The owner likes green tea.", "memories": [{"action": "create", "content": "The owner likes green tea.", "importance": 3, "temporal": "enduring"}]}"#,
    );
    let release = transport.park_learning();
    say(&mut desktop, "I like green tea").await;
    let before = desktop.ui_ticks();
    for _ in 0..8 {
        desktop.tick();
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    assert_eq!(
        desktop.ui_ticks() - before,
        8,
        "each tick must advance the counter while learning waits"
    );
    desktop.refresh_memory().await.expect("list during park");
    assert!(
        desktop
            .memory()
            .rows()
            .iter()
            .all(|row| !row.content.contains("green tea")),
        "the GUI must not invent a Memory while formation is parked: {:?}",
        desktop.memory().rows()
    );
    release.send(()).expect("release learning");
    transport.wait_learning().await;
    wait_for_memory(&mut desktop, "The owner likes green tea.").await;
    desktop.open_page(Page::Memory);
    assert_eq!(desktop.snapshot().page, "Memory");

    server.shutdown_and_join().await;
}
