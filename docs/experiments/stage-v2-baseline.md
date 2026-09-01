# Stage v2 pre-migration baseline

Tracking: [#1261](https://github.com/pexisgle/ene/issues/1261).
Japanese: [ja/experiments/stage-v2-baseline.md](../ja/experiments/stage-v2-baseline.md).
Decisions: [stage-ui-poc-findings.md](stage-ui-poc-findings.md).

This page freezes the pre-Slint Stage so later PRs can tell a regression from a
bug that already existed. It does not change the renderer.

## Commit under test

Recorded from `cursor/stage-v2-slint-migration-345a` branched from `origin/main`
at `8d970ce8` (`fix(ci): drop unused Windows tray imports and rustdoc crate link`).

## Event-loop / idle redraw (code fact)

`ene-stage` currently sets `winit::event_loop::ControlFlow::Poll` in
`app::run` and again in `StageApp::about_to_wait`. That callback always calls
`tick_overlay` and `paint_chrome`.

Consequence, independent of GPU:

- The process does **not** idle at 0 frames today.
- “idle CPU ≈ 0 / 0 frames when unchanged” is a Stage v2 **target**, not the
  current measurement.
- Overlay VRM with an idle VRMA or spring bones will keep requesting frames
  even after WaitUntil lands; a static pose with no visemes, look-at, or dirty
  Slint UI must not.

## `cargo test -p ene-stage`

Same-commit repeats on the Cloud Agent VM (Linux, rustup stable, no Nix).
Software Vulkan (lavapipe) is available for GUI, but these runs are unit tests
only.

| Run | Result | Notes |
|---|---|---|
| 1 | 273 passed, 0 failed (`cargo test -p ene-stage`) | Cloud Agent VM, rustup stable. No flake on this run. |
| 2 | 273 passed, 0 failed | Same commit, repeat. |
| 3 | 273 passed, 0 failed | Same commit, repeat. |

Suspect async-timing tests (names to watch; do **not** mass-fix in Stage v2 PRs):

- `ene_stage::app::tests` cases that drain `AsyncOutcome` and wait on session /
  history reconciliation (`stale_*`, `reconciliation_*`, `completion_*`).
- `ene_stage::detail::tests` cases that ignore stale MCP / job results.

If a later PR makes one of these fail **every** run on this commit’s successors,
that is a new regression. Intermittent failures at a similar rate are the
baseline.

## Performance (record, do not gate on software GPU)

| Metric | Windows (real GPU) | Linux (real GPU) | Cloud VM (lavapipe) |
|---|---|---|---|
| Startup to first overlay frame | measure on #1261 host | measure on #1261 host | software-reference only |
| Idle CPU while Stage is up, no pointer, no speech | measure | measure | expected non-zero because of `ControlFlow::Poll` |
| RSS after load + two bundled avatars | measure | measure | software-reference only |
| VRM steady-state frame time (idle motion) | measure p50/p95/p99 | measure | software-reference only |
| Idle redraw when overlay is hidden / no avatars | should still Poll today | same | same |

Fill the real-GPU columns from a developer machine when available. Do not fail
#1265 / #1273 / #1281 solely on lavapipe deltas.

## Platform input today

| Platform | Overlay click-through | Hover re-arm | Notes |
|---|---|---|---|
| Windows | window-wide `Window::set_cursor_hittest` (winit → `WS_EX_TRANSPARENT`) | none in Stage; Passive windows get no pointer | Keep this architecture. |
| X11 | `set_cursor_hittest` is a no-op; `cursor_poll` queries the root pointer | yes, 50 ms poll | Stage v2 replaces this with coarse SHAPE where it sticks. |
| Wayland | no input region; click-through means no pointer events | none | Users turn click-through off to drag. Stage v2 adds `set_input_region`. |

`platform::apply_click_through` (manual HWND `EXSTYLE`) is unused. Production
mutates hit-test through `StageInteractionController` and `OverlayPlatform`.

## Regression checklist for later issues

Use this list on #1265, #1269, #1273, and #1281:

1. Transparent overlay still presents premultiplied alpha, or hides when unsupported.
2. Always-on-top still follows `desktop.always_on_top` and chrome-focus lowering.
3. Windows Passive still lets the desktop behind receive clicks.
4. Windows Interactive / Dragging / UiFocused still receive the first pointer event.
5. Wayland: clicks outside `InteractionGeometry` do not hit Stage (Weston 13; other compositors unclaimed).
6. X11: UI/VRM clicks reach Stage on a supported WM; visual-only / background clicks can reach another process; no ShapeNotify fight.
7. VRM drag, click, double-click, long-press still classify through `GestureTracker`.
8. Display-only overlay UI does not force Interactive.
9. Resize and DPI do not leave a black or clipped swapchain.
10. Two displayed avatars still load, hit-test, and persist positions.
11. After WaitUntil: unchanged static Stage does not spin the event loop.
12. RSS / frame-time deltas vs this page are recorded (real GPU).
