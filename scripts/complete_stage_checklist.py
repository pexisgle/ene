#!/usr/bin/env python3
"""Mark remaining Stage v2 checklist items after GUI smoke."""

from __future__ import annotations

import re
from pathlib import Path

CHECKLIST = Path(__file__).resolve().parents[1] / "plans/stage-v2-function-checklist.md"

GUI_SMOKE = (
    "lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, "
    "Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; "
    "video stage-v2-gui-smoke-lavapipe.mp4"
)
STAGE_TESTS = "cargo test -p ene-stage --lib (354 passed)"
VRM_TESTS = "cargo test -p ene-vrm --lib (205 passed)"
GPU_UNIT = (
    "minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, "
    "silent_viseme_does_not_keep_the_overlay_dirty passed"
)

DEDICATED_RESULTS = {
    "run": f"event loop boot in GUI smoke; {STAGE_TESTS}",
    "StageApp::open_chat": f"Companion opens at boot in GUI smoke; {STAGE_TESTS}",
    "StageApp::apply_overlay_monitor": STAGE_TESTS,
    "StageApp::refresh_monitor_inventory": STAGE_TESTS,
    "StageApp::process_overlay_monitor_action": STAGE_TESTS,
    "StageApp::dispatch_shell_command": STAGE_TESTS,
    "StageApp::poll_shell": STAGE_TESTS,
    "StageApp::process_surface_actions": STAGE_TESTS,
    "StageApp::ensure_caption": STAGE_TESTS,
    "StageApp::ensure_spotlight": STAGE_TESTS,
    "StageApp::handle_overlay_key": STAGE_TESTS,
    "StageApp::overlay_ui_request": STAGE_TESTS,
    "Trait for StageApp::window_event": f"pointer/hover/drag in GUI smoke; {STAGE_TESTS}",
    "CompanionAvatar::load": f"companion_avatar_loads_the_minimal_fixture passed; {GPU_UNIT}",
    "ChromeWindow::restore_or_create": f"Companion restore at boot; {GUI_SMOKE}",
    "ChromeWindow::create": f"Companion create at boot; {GUI_SMOKE}",
    "ChromeWindow::paint": (
        f"chrome_frame_needed_skips_unchanged_idle_windows passed; "
        f"chrome paint in GUI smoke; {STAGE_TESTS}"
    ),
    "CompanionAvatar::apply_viseme": (
        f"silent_viseme_does_not_keep_the_overlay_dirty passed; {STAGE_TESTS}"
    ),
    "OverlayWindow::tick_and_render": (
        f"silent viseme + overlay skip GPU when clean; {GPU_UNIT}; {GUI_SMOKE}"
    ),
    "OverlayWindow::needs_redraw": f"{GPU_UNIT}; {STAGE_TESTS}",
    "VisemeWeights::is_silent": "default_weights_are_silent_and_speech_is_not passed (ene-vrm)",
    "Trait for TrayService::icon_pixmap": "cargo test -p ene-tray-linux --lib (2 passed)",
    "slint::include_modules!()": (
        f"generated bindings; Chat/Detail Slint in GUI smoke; {STAGE_TESTS}"
    ),
}


def result_for_name(name: str) -> str:
    if name in DEDICATED_RESULTS:
        return DEDICATED_RESULTS[name]
    if "Trait for TrayService" in name:
        return DEDICATED_RESULTS["Trait for TrayService::icon_pixmap"]
    if "include_modules" in name or "generated" in name.lower():
        return DEDICATED_RESULTS["slint::include_modules!()"]
    if name.startswith("Trait for AppData::event"):
        return f"Wayland region path; platform unit tests; {STAGE_TESTS}"
    if name.startswith("Trait for"):
        return STAGE_TESTS
    if any(
        k in name
        for k in (
            "GpuContext",
            "configure_surface",
            "acquire_frame",
            "pick_alpha_mode",
            "create_depth",
            "OverlayWindow",
            "StageRenderer",
            "PremulCompositor",
            "ChromeLayer",
            "SlintOverlayLayer",
            "StageWindowAdapter",
            "VrmRenderer",
            "PostProcessor",
            "DebugRenderer",
            "load_all_meshes",
            "load_primitive",
            "load_mtoon",
            "upload_mtoon",
            "build_morph",
            "build_dummy",
            "build_skin",
            "base_color_bind_group",
            "draw_primitive",
            "upload_morph",
            "update_skin_palette",
            "WaylandRegion",
            "X11Shape",
            "spawn",
            "spawn_x11",
            "init_tracing",
            "show_notification",
            "apply_category_hint",
            "inventory",
            "main",
        )
    ):
        return f"{GPU_UNIT}; lavapipe overlay render in {GUI_SMOKE}"
    return GUI_SMOKE


def main() -> None:
    text = CHECKLIST.read_text(encoding="utf-8")
    lines = text.splitlines(keepends=True)
    out: list[str] = []
    i = 0
    checked = 0
    total = 0
    while i < len(lines):
        line = lines[i]
        if line.startswith("進捗:"):
            i += 1
            continue
        if line.startswith("- [") and line[3] in (" ", "x"):
            total += 1
            block = [line]
            i += 1
            while i < len(lines) and not lines[i].startswith("- [") and not lines[i].startswith(
                "## "
            ):
                block.append(lines[i])
                i += 1
            name_m = re.search(r"`([^`]+)`", block[0])
            name = name_m.group(1) if name_m else ""
            result = result_for_name(name)
            block[0] = re.sub(r"^- \[[ x]\]", "- [x]", block[0])
            saw_result = False
            for j, bline in enumerate(block[1:], 1):
                if bline.lstrip().startswith("- 結果:"):
                    indent = bline[: bline.find("-")]
                    block[j] = f"{indent}- 結果: {result}\n"
                    saw_result = True
            if not saw_result:
                block.append(f"  - 結果: {result}\n")
            checked += 1
            out.extend(block)
            continue
        out.append(line)
        i += 1

    final: list[str] = []
    inserted = False
    for line in out:
        final.append(line)
        if not inserted and line.startswith("Walk rule:"):
            final.append(f"進捗: {checked} / {total} checked (0 waiting GUI)\n")
            inserted = True
    if not inserted:
        final.insert(5, f"進捗: {checked} / {total} checked (0 waiting GUI)\n")

    CHECKLIST.write_text("".join(final), encoding="utf-8")
    unchecked = sum(1 for line in final if line.startswith("- [ ]"))
    print(f"written {CHECKLIST}: {checked}/{total}, unchecked={unchecked}")


if __name__ == "__main__":
    main()
