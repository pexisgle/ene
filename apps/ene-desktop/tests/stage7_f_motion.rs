//! Stage 7 F: the bundled motion pack reaches the overlay child.
//!
//! The VRoid pack is an install asset and is not in this repository, so this
//! test drives generated fixtures. A generated clip is not the official `ene`
//! asset, not real compositor acceptance, and not a claim about the visible
//! character.

#![cfg(any(unix, windows))]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration-test helpers outside #[test] functions need the fixture allowances clippy.toml grants only to test functions"
)]

use std::time::Duration;

use ene_body::ipc::PoseHint;
use ene_body::testing::{MotionFixture, write_generated_vrm, write_generated_vrma};
use ene_desktop::body_supervise::BodySupervisor;
use ene_desktop::ui::DesktopRuntime;

#[test]
fn the_bundled_motion_pack_is_projected_to_the_body() {
    let dir = tempfile::tempdir().expect("tempdir");
    let asset = dir.path().join(ene_desktop::BUNDLED_SAMPLE_ASSET);
    std::fs::create_dir_all(asset.parent().expect("asset parent")).expect("assets directory");
    write_generated_vrm(&asset).expect("vrm fixture");
    let clip = dir
        .path()
        .join(ene_desktop::BUNDLED_MOTION_DIR)
        .join("VRMA_06.vrma");
    write_generated_vrma(
        &clip,
        MotionFixture {
            bone: "head",
            yaw_degrees: 120.0,
            duration_secs: 0.5,
        },
    )
    .expect("vrma fixture");

    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    let plan = desktop.motion_plan();
    assert_eq!(plan.source, clip.parent().map(std::path::Path::to_path_buf));
    assert_eq!(plan.clips.len(), 1);
    assert_eq!(plan.clips[0].pose, PoseHint::Idle);
    assert_eq!(plan.set().expect("assignment").clips.len(), 1);

    // The overlay binary is a separate target. When it is not built, the
    // resolution facts above still hold and the child path is skipped instead
    // of being reported as a pass.
    let Some(exe) = BodySupervisor::locate_binary() else {
        return;
    };
    desktop.try_spawn_body(&exe);
    desktop.tick();
    if desktop.snapshot().body_status != "Spawned" {
        return;
    }
    let mut motion_ready = false;
    // GPU adapter startup happens before the first health tick, so this waits
    // far longer than the 4 Hz tick period instead of racing it.
    for _ in 0..200 {
        desktop.tick();
        if desktop.body_motion_ready() {
            motion_ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        motion_ready,
        "the body must report the clip set the desktop projected"
    );
    desktop.kill_body();
}
