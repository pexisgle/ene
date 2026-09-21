# Windows 11 native acceptance — 2026-09-21

## Verdict

**NOT ACCEPTED.** The tested `main` release fails first-run model assignment and native avatar presentation. A separate local repair branch unblocks setup, real API chat and DirectComposition transparency. Click-through, official-character rendering and a measured cancel-to-paint deadline remain unaccepted. Native drag/resize and changed HiDPI scales could not be exercised through the available Windows UI tool.

This report distinguishes observed success, observed failure and missing evidence. Unit-test success, a live HWND, submitted GPU frames and a successful command response are not substitutes for native acceptance.

## Source and reproducibility

| Build | Exact source | Role |
|---|---|---|
| Clean `main` release | `515c7e0c8e206e9c8f2ac8a1bc2e91945dd55a86` | Baseline, before repairs |
| Local repaired release | `a21fe8a5a853d30afc489777e9f36150623237cf` | Final 300-second measurement |

Checkout: `C:\dev\ene-windows-acceptance`, branch `codex/windows-acceptance-20260921`. The original checkout at `C:\dev\ene` was not changed. Build command: `cargo build --release --workspace --locked`. Toolchain: rustc 1.98.1, Cargo 1.98.1, x86_64-pc-windows-msvc. Baseline release build took 7m11s; final incremental release build took 58.27s. Binary SHA-256 manifests are retained separately for baseline and final release.

Functional repair trials `fix1`–`fix3` were exploratory release builds of the working tree before the repair commit. They must not be described as clean-main results. `20260921-final/build.json` identifies the exact committed source and binaries for the final measurement.

Raw evidence root: `C:\dev\ene-acceptance-evidence`. Relative paths below resolve under this root. `evidence-manifest.json` records sizes and SHA-256 values. The dedicated runtime profile, `.env`, credentials, and task-workspace contents are excluded from that manifest and from any shareable evidence set. Do not publish the whole evidence-root directory: it also contains the private test profile.

## Environment and asset

- Windows 11 Pro x64, version 10.0.26200, DWM desktop.
- AMD Ryzen 5 5600, 6 physical / 12 logical processors.
- AMD Radeon RX 9060 XT, driver 32.0.31041.1004; 1920×1080, 60 Hz.
- GUI: Slint 1.18.0, winit, `skia-software`. Repaired Body: wgpu 27, DX12 `DxgiFromVisual` DirectComposition surface.
- UI language English for repaired trials; Japanese IME verified with key events. Display scale was not independently verified or changed; no 150%/200% result is claimed.
- Dedicated `ENE_DATA_DIR` and synthetic workspace. The user's `.env` key was entered into the real credential-registration UI and approved through the Host control surface. No key value is recorded. Provider preflight returned HTTP 200 for `gpt-4.1` metadata; actual GUI dialogue subsequently received a Japanese reply.
- Normal interactive desktop, with unrelated user applications still open. This was not a clean-boot or isolated GPU benchmark. No compilation ran during the final 300-second sample.

The user requested the asset from “PR1761”. That PR number was not found; matching asset PR [#1671](https://github.com/pexisgle/ene/pull/1671), head `8bca63f1b0d419c8ee5fa8eba0215f8f50aac699`, describes **Seed-san**, not the official ene character. The downloaded official VRM sample matched its documented hash exactly:

`624d0d554bc205bbdc33e22a68a2c3c20edebb3e573011ead8878a65e5329b23`

Credit: VirtualCast, Inc.; VRM Public License 1.0. The strict VRM1 asset probe succeeded with 21 primitives, 18 expressions and 9 spring chains. The sample is a technical probe only. The official ene asset remains absent; no official-character performance or visual acceptance is claimed. The initially discovered local Alicia asset in environment discovery was VRM0 and was not used for the campaign.

## Acceptance matrix

| Requested check | Clean main | Repaired trials / final measurement |
|---|---|---|
| Native overlay really appears | FAIL: opaque white window; no avatar/display CSV | Avatar rendered through DirectComposition; transparent background visually confirmed. Sample mesh has separated arm fragments; visual quality remains failed |
| Transparency | FAIL | Observed after removing DWM frame extension and GDI redirection; screenshot retained |
| Click-through | Not accepted | FAIL: a visually empty area can still intercept the underlying settings control; no successful compositor-level click-through record |
| Drag / resize | Blocked by baseline rendering | NOT VERIFIED natively. Body toolwindow is absent from tool enumeration. Resize hit-test ordering and GPU extent synchronization were repaired in source, but this is not a native interaction pass |
| Hide → restore | Baseline blocked | Observed in `fix3`: Settings hide removes avatar; Show redraws it without restarting GUI or Body |
| HiDPI / scale changes | Not exercised | NOT VERIFIED. Settings app launch exposed no targetable window; no scale mutation or simulated scale result substituted |
| Body crash: chat / settings / cancel | Same GUI settings survived; chat blocked by setup | Observed in `fix3`: same Host/GUI PIDs survive forced Body termination, actual API task delegation still works, task cancels, Settings opens and reports avatar unavailable |
| Japanese IME / English UI | English language switch after Body termination | Observed actual key input `a`, `i`, Space, Enter → `愛`; candidate/preedit and committed text captured. IME commit did not submit chat |
| PresentMon ↔ ene-measure | No display events from broken baseline Body | Final measurement uses real v2 metrics, exact Body PID and swap-chain identity; see results below |
| 5-minute CPU / RSS | No valid rendered idle baseline | Final 300-second sample below; Windows memory metric is Working Set |
| cancel → Host intake → GUI paint ≤1s | Not measured | INCOMPLETE: functional cancellation succeeds but software renderer produces no AfterRendering trace; no deadline pass claimed |

## Observed repairs and remaining problems

1. **First assignment deadlock.** The Host's credential projection describes the assigned dialogue route. Before first assignment it remains false, even after successful key registration. The wizard required this same flag to enable assignment. The UI now allows the assignment attempt with a model name; Host credential/consent validation remains authoritative. Baseline evidence: `20260921-main/02-credential-not-ready.jpg` and `.txt`.
2. **Wrong production credential store.** The Host published OS-backed credentials while the production OpenAI transport used a separate environment store. The transport now borrows the Host's published snapshot, including rotation/deactivation. A focused regression test verifies initial absence, publication, rotation, rejected transport writes and deactivation. GUI received `こんにちは！`; see `20260921-fix2/chat-success.jpg` and `.txt`.
3. **Opaque HWND swap-chain path.** Default DX12 HWND surface capabilities did not satisfy the renderer's alpha requirement. Windows now uses `DxgiFromVisual`, `WS_EX_NOREDIRECTIONBITMAP`, and no full-client `DwmExtendFrameIntoClientArea`. The combination rendered transparent pixels successfully. See `20260921-main-seed/03-overlay-white.jpg` versus `20260921-fix3/transparent-overlay.jpg`.
4. **Resize defects.** The bottom-right resize grip was tested only after the elliptical transparent-area rejection, making the grip unreachable. It is now checked first. `WM_SIZE` updated placement before the renderer-size comparison, so GPU resize could be skipped; the renderer now compares its own physical extent and avoids redundant reconfiguration. Native drag/resize still requires real input evidence.
5. **PresentMon column mismatch.** PresentMon 2.5.1's default output is not the importer's expected v2 schema. `ene-measure` now explicitly requests `--v2_metrics`. The default-schema and explicit-v2 probe CSVs are retained. No fabricated displayed-duration columns are inserted.
6. **Remaining hit-test limitation.** The current ellipse is much larger than visible model pixels. A click intended for Settings General at screen `(338,383)` was rejected by the UI tool because Body owned the point. This is observed input interception, not proof of successful click-through. The tool cannot force input through another process; that restriction was not bypassed.
7. **Remaining visual issues.** Seed-san displays separated arm/accessory-like geometry. Settings also has persistent white margins and initially incomplete-looking content in captures. These are retained as visual failures; neither is explained away as successful rendering.
8. **Missing paint timing.** Slint skia's software surface uses the default no-op `with_graphics_api`; the renderer's `AfterRendering` callback is invoked through that method. Installing the notifier can succeed without ever producing the measurement event. `ENE_INTERACTION_TRACE_JSONL` was configured, but no trace file appeared. A timer, screenshot observation time, or request completion was not substituted for GUI paint.

`host_intake_monotonic_ns` also currently names a GUI-side observation of the Host outcome, including follow-up refresh work, not an instrumented Host intake instant. If available it would be an upper bound. All current interaction fields use the GUI process's monotonic clock; independent process clocks must not be subtracted. The one-second requirement remains blocked pending a real software-paint evidence path and clearly defined intake instrumentation.

## Crash and input evidence

In `20260921-fix3`, Body PID **32516** was forcibly terminated. Host PID **23300** and GUI PID **10328** remained identical before and after the failure. `before-crash.json`, `after-crash.json` and `crash-time.txt` retain this evidence. No Host or GUI restart was used to demonstrate survival.

After Body termination, the GUI delegated a synthetic long-writing task through the real API, selected the running task and cancelled it. `crash-cancel-state.jpg/.txt` shows `Cancelled` alongside its specific `crash-cancel-probe.md` purpose. `crash-settings.jpg/.txt` shows the same GUI's unavailable-avatar notice. Before the crash, `cancel-state.jpg/.txt` records a separate normal cancellation. These are functional trials, not a latency distribution or three repeated deadline trials.

`hide.jpg` and `restore.jpg` retain the hide/restore comparison. `ime-conversion.jpg`, `ime-commit.jpg` and `ime-commit.txt` retain native IME evidence. Japanese Unicode paste was not used as the IME acceptance test. Unsaved IME test text was cleared afterward.

## Performance and correlation

Final source: `a21fe8a5a853d30afc489777e9f36150623237cf`. Host PID **27376**, GUI PID **33124**, Body PID **31976**. All three were sampled 301 times over **300.0002024 seconds**, nominal interval 1 second. PresentMon 2.5.1 recorded the Body swap chain **`0x1BFC0113760`** concurrently. The first 5 seconds are FPS warmup; the fixed FPS window is CPU start time `[5,65)` seconds, denominator 60 seconds.

| Metric | Result | Individual numerical gate |
|---|---:|---|
| Total CPU time | 27.218750 seconds | — |
| CPU, normalized to 12 logical CPUs | **0.756076%** | ≤10%: pass for this sample |
| Sum of core equivalents | 0.090729 | No process reaches the 0.5-core busy-wait threshold |
| Mean summed Working Set | 204,238,997 bytes / 194.778 MiB | — |
| Peak simultaneous summed Working Set | **207,405,056 bytes / 197.797 MiB** | ≤2 GiB: pass for this sample |
| Displayed frames in fixed 60s | **1,800** | — |
| Discarded / missing frames in fixed 60s | **0 / 0** | No missing evidence in this window |
| Actual display FPS | **30.000** | ≥30: pass for this sample |

| Process | CPU seconds | One-core equivalent | Mean / peak Working Set bytes |
|---|---:|---:|---:|
| Host | 0.000000 | 0.000000 | 13,934,251 / 14,020,608 |
| GUI | 0.515625 | 0.001719 | 70,905,148 / 71,049,216 |
| Body | 26.703125 | 0.089010 | 119,399,597 / 122,363,904 |

The full PresentMon file contains **8,996** rows matching the exact PID/swap chain. `correlate.mjs` independently counted the fixed-window displayed/discarded/missing rows and reconstructed display times from `CPUStartTime + DisplayLatency`. Its 1,800/0/0 counts and 30 FPS match `ene-measure`. Independent CPU deltas from raw process points and the maximum simultaneous Working Set sum also match exactly; see `20260921-final/independent-check.json` and `correlation.json`.

Raw results: `20260921-final/measurement.json` (all process points and presentation events), `measurement.md`, `presentmon.csv`, `measure.log`, `measurement-environment.json`, `processes.json`, `build.json`. `measure-final.ps1` preserves the exact invocation. The program exits **2** and reports **Incomplete**, with click-through unmeasured; interaction samples are also empty. This is not an overall performance-acceptance pass and does not establish the cancel deadline or official-character performance.

The instrument intentionally receives no invented interaction or click-through input. An overall non-pass result is expected even when an individual CPU/RSS limit passes. Asset identity, unverified scale, click-through failure and missing paint evidence remain independent acceptance blockers.

## Validation

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass after Windows-only lint fixes.
- `cargo test --workspace --locked`: **1,493 passed**, zero failures, including the published-credential regression.
- `cargo build --release --workspace --locked`: pass for baseline and repaired source.
- MSVC's Japanese informational “creating library / export object” stdout is surfaced by Rust as `linker_messages` warnings in build/test logs. Setting `VSLANG=1033` did not remove them. No warning-suppression attribute was added; the Clippy gate itself passed.

Logs: `fmt.log`, `clippy.log`, `tests.log`, `build-final.log`, `validation-summary.json`. The initial baseline logs/hashes and the intermediate failure evidence have not been overwritten by the final measurement.

## Evidence handling and follow-up

UI control used the installed computer-use skill's Windows `@oai/sky` surface. Body's native toolwindow was not enumerated, and Windows Settings did not expose a targetable window. No alternate native UI automation or registry-based scale simulation was used. Manual drag/resize assistance was requested; no observation was received during the recorded run.

To finish acceptance: correct and natively verify transparent input regions; resolve the sample mesh/Settings rendering issues; exercise real drag/resize and 100%/150%/200% display transitions; supply the official ene asset; provide actual software-paint/intake timing; then rerun performance and repeated cancellation trials at one exact release SHA.

Primary references: [Microsoft DirectComposition architecture](https://github.com/MicrosoftDocs/win32/blob/docs/desktop-src/directcomp/architecture-and-components.md), [DWM custom frame behavior](https://learn.microsoft.com/en-us/windows/win32/dwm/customframe), and [PresentMon console documentation](https://github.com/GameTechDev/PresentMon/blob/main/README-ConsoleApplication.md). Behavior claims above are grounded in the local source and captured Windows run, not inferred from those documents alone.
