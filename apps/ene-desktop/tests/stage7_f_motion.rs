#![cfg(any(unix, windows))]
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

    let Some(exe) = BodySupervisor::locate_binary() else {
        return;
    };
    desktop.try_spawn_body(&exe);
    desktop.tick();
    if desktop.snapshot().body_status != "Spawned" {
        return;
    }
    let mut motion_ready = false;
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
