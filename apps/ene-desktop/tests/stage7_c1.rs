#![cfg(any(unix, windows))]

use std::collections::VecDeque;
use std::future::Future;
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
use ene_desktop::session::{self, ChatSendReport};
use ene_desktop::ui::{DesktopRuntime, Page};
use ene_inference::{ProviderRequest, ProviderResponse, ProviderTransport};

mod common;

use common::{ServingTask, open_host, pair_and_seat, wait_for_control};

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
}

impl ProviderTransport for LearningTransport {
    fn complete_streaming<'a>(
        &'a self,
        req: ProviderRequest,
        sink: &'a mut (dyn ene_inference::DeltaSink + Send),
    ) -> Pin<
        Box<
            dyn Future<Output = Result<ProviderResponse, ene_inference::InferenceTechnicalError>>
                + Send
                + 'a,
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
                return match sink.push_delta(&answer).await {
                    ene_inference::DeltaFlow::Continue => Ok(ProviderResponse {
                        text: answer,
                        usage: None,
                    }),
                    ene_inference::DeltaFlow::Abort(reason) => {
                        Err(ene_inference::InferenceTechnicalError::StreamAborted {
                            reason: reason.to_owned(),
                        })
                    }
                };
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
            let response = ProviderResponse {
                text: reply,
                usage: None,
            };
            match sink.push_delta(&response.text).await {
                ene_inference::DeltaFlow::Continue => Ok(response),
                ene_inference::DeltaFlow::Abort(reason) => {
                    Err(ene_inference::InferenceTechnicalError::StreamAborted {
                        reason: reason.to_owned(),
                    })
                }
            }
        })
    }
}

async fn complete_setup(desktop: &mut DesktopRuntime) {
    common::complete_setup(desktop, SECRET, MODEL).await;
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
    let report = desktop.send_text().await.expect("chat turn must complete");
    assert!(matches!(report, ChatSendReport::Completed));
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
    desktop.page_older_memories().await.expect("next Host page");
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
    for _ in 0..8 {
        desktop.tick();
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
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
