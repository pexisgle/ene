#![cfg(any(unix, windows))]

use std::collections::{BTreeSet, VecDeque};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ene_api::v1::management::ManagementOutcome;
use ene_desktop::ui::{DesktopRuntime, Page};
use ene_inference::{ProviderRequest, ProviderResponse, ProviderTransport};

mod common;

use common::{ServingTask, open_host, pair_and_seat, wait_for_control};

const MODEL: &str = "gpt-slice-test";
const SECRET: &str = "sk-stage7-c2-secret-4408";

const PROPOSE_REPLY: &str =
    r#"[task-control] {"kind":"propose_task","purpose":"read input.txt and write report.md"}"#;
const READ_REPLY: &str = r#"{"tool":"read","path":"input.txt"}"#;
const CREATE_REPLY: &str =
    "{\"tool\":\"create\",\"path\":\"report.md\",\"content\":\"# Report\\nnotes\"}";
const FINAL_REPLY: &str = r#"{"final":"created report.md from input.txt"}"#;

struct GateTransport {
    replies: Mutex<VecDeque<String>>,
    sends: AtomicUsize,
    blocks: Mutex<BTreeSet<usize>>,
    failures: Mutex<BTreeSet<usize>>,
}

impl GateTransport {
    fn new(replies: Vec<String>, blocks: &[usize]) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            sends: AtomicUsize::new(0),
            blocks: Mutex::new(blocks.iter().copied().collect()),
            failures: Mutex::new(BTreeSet::new()),
        })
    }

    fn sends(&self) -> usize {
        self.sends.load(Ordering::SeqCst)
    }

    fn unblock(&self, call: usize) {
        self.blocks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&call);
    }

    async fn wait_sends(&self, wanted: usize) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while self.sends() < wanted {
            assert!(
                tokio::time::Instant::now() < deadline,
                "provider sends did not reach {wanted}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

impl ProviderTransport for GateTransport {
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
        let _ = req;
        Box::pin(async move {
            let call = self.sends.fetch_add(1, Ordering::SeqCst) + 1;
            loop {
                let held = self
                    .blocks
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .contains(&call);
                if !held {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            if self
                .failures
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&call)
            {
                return Err(
                    ene_inference::InferenceTechnicalError::ProviderTransportFailed(String::from(
                        "fixture provider disconnected before returning a result",
                    )),
                );
            }
            let text = self
                .replies
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop_front()
                .unwrap_or_default();
            let response = ProviderResponse { text, usage: None };
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
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
async fn say(desktop: &mut DesktopRuntime, text: &str) {
    desktop.composer_mut().set_draft(text.to_owned());
    desktop.send_text().await.expect("chat turn must complete");
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
fn workspace_with_input() -> tempfile::TempDir {
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    workspace
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
async fn wait_listed(desktop: &mut DesktopRuntime, needle: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        desktop.refresh_tasks().await.expect("task list");
        let snap = desktop.snapshot();
        if snap.tasks.iter().any(|line| line.contains(needle)) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "task list never showed {needle:?}: {:?}",
            snap.tasks
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_path(path: &Path) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !path.exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "workspace write did not land: {}",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
async fn select_first_task(desktop: &mut DesktopRuntime) {
    desktop.open_tasks().await.expect("open tasks");
    desktop
        .select_listed_task(0)
        .await
        .expect("select listed task");
}

#[tokio::test]
async fn acceptance_4_workspace_task_gui_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = workspace_with_input();
    let transport = GateTransport::new(
        vec![
            String::from(PROPOSE_REPLY),
            String::from("You are welcome."),
            String::from(READ_REPLY),
            String::from(CREATE_REPLY),
            String::from(FINAL_REPLY),
        ],
        &[2],
    );
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_seat(&mut desktop, &handle).await;
    complete_setup(&mut desktop).await;
    desktop.try_spawn_body(&dir.path().join("ene-body-absent"));
    assert_eq!(desktop.snapshot().body_status, "Absent");

    desktop.open_tasks().await.expect("empty tasks page");
    assert_eq!(desktop.snapshot().page, "Tasks");
    assert!(
        desktop.snapshot().tasks.is_empty(),
        "the GUI must not mint tasks: {:?}",
        desktop.snapshot().tasks
    );

    let selected = desktop
        .select_workspace_folder(workspace.path())
        .await
        .expect("workspace select");
    assert!(
        matches!(selected, ManagementOutcome::AppliedAsOneTime),
        "workspace must apply, got {selected:?}"
    );

    say(
        &mut desktop,
        "please read input.txt and write report.md as a task",
    )
    .await;
    let timeline = desktop.snapshot().timeline.join("\n");
    assert!(
        timeline.contains("Task accepted"),
        "creation stays on companion delegation: {timeline}"
    );
    assert!(
        !timeline.contains("[task-control]"),
        "protocol must not appear: {timeline}"
    );
    transport.wait_sends(2).await;

    wait_listed(&mut desktop, "in_progress").await;
    wait_listed(&mut desktop, " running").await;
    select_first_task(&mut desktop).await;
    let detail = desktop.snapshot().task_detail;
    assert!(detail.contains("rev 1"), "{detail}");
    assert!(detail.contains("in_progress"), "{detail}");
    assert!(detail.contains("running=yes"), "{detail}");
    assert!(detail.contains("interrupted=false"), "{detail}");
    assert!(
        detail.contains("purpose-id ") && detail.contains(':'),
        "purpose identity is Host-published: {detail}"
    );
    assert!(
        detail.contains("workspace ")
            && detail.contains(&workspace.path().to_string_lossy().into_owned()),
        "workspace path is the Owner-sent folder: {detail}"
    );
    assert!(detail.contains("actions "), "{detail}");
    assert!(
        !desktop.snapshot().contains_secret(SECRET),
        "registered secret must not appear in the task snapshot"
    );

    desktop.open_page(Page::Chat);
    say(&mut desktop, "thanks, keep going").await;
    assert_eq!(
        desktop
            .snapshot()
            .timeline
            .iter()
            .filter(|line| line.contains("You are welcome."))
            .count(),
        1
    );
    desktop.open_tasks().await.expect("tasks while running");
    assert!(
        desktop
            .snapshot()
            .tasks
            .iter()
            .any(|line| line.contains("in_progress")),
        "parallel chat must not cancel the Host-only Task: {:?}",
        desktop.snapshot().tasks
    );

    transport.unblock(2);
    wait_path(&workspace.path().join("report.md")).await;
    wait_listed(&mut desktop, "completed").await;
    select_first_task(&mut desktop).await;
    let detail = desktop.snapshot().task_detail;
    assert!(detail.contains("completed"), "{detail}");
    assert!(detail.contains("running=no"), "{detail}");
    assert!(detail.contains("interrupted=false"), "{detail}");
    assert!(
        detail.contains("adopted-rev") || detail.contains("task_result"),
        "result adoption is Host-authored: {detail}"
    );
    assert!(
        detail.contains("report.md") || detail.contains("created report.md"),
        "changed files / remaining work come from the report source: {detail}"
    );
    assert!(workspace.path().join("input.txt").exists());
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("report.md")).unwrap(),
        "# Report\nnotes"
    );
    assert_eq!(transport.sends(), 5, "no second launch");
    server.shutdown_and_join().await;
}
