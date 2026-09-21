# Stage 7 Windows D/F acceptance — 2026-09-22

## Conclusion

**WINDOWS D/F ACCEPTANCE COMPLETE EXCEPT OFFICIAL ENE ASSET**

PR #1669後のWindows 11 production pathを実機検証し、Windows D/F runtime / platform
acceptanceは完了した。Windows側の残件は公式`ene` VRM（#1651）だけである。

## Tested revision

- base `main`: `5bc0b12818b65ea800a43aa9b0de1beafdf53fb2`
- tested implementation: 上記commitに対する本修正

## Environment

- Windows 11 Pro x64、build 26200
- AMD Ryzen 5 5600、6 cores / 12 threads
- AMD Radeon RX 9060 XT、driver 32.0.31041.1004
- 1920x1080 @ 60 Hz x2、100% / temporary 125% mixed-DPI
- Rust / Cargo 1.98.1、MSVC
- wgpu 27 / DX12 / DirectComposition
- Slint 1.18.0 / winit software renderer
- PresentMon 2.5.1

## Automated verification

| Gate | Result |
|---|---|
| Format | PASS |
| `git diff --check` | PASS |
| Workspace build | PASS |
| Clippy `-D warnings` | PASS |
| Workspace tests | PASS、1,524 tests / 73 suites |
| Workspace docs | PASS |
| Release build | PASS |

## Windows D/F acceptance

| Item | Result |
|---|---|
| Native DWM / DirectComposition + real wgpu surface | PASS |
| Transparent background / avatar-only rendering | PASS |
| Cross-process click-through / desktop blocking | PASS |
| Drag / resize / hide / restore | PASS |
| Surface and desktop-state recovery | PASS |
| 100%↔125% mixed-DPI monitor transition | PASS |
| Seed-san VRM 1.0 load | PASS |
| Humanoid / expression / LookAt / SpringBone | PASS |
| Continuous actual rendered frames | PASS |
| Resize / DPI change / Body restart recovery | PASS |
| Actual presented FPS / CPU / memory | PASS（observed varianceはOwner判断でaccept） |
| Operation input → Host outcome → GUI paint | PASS、6.7 ms / 16.1 ms |
| Body / renderer failure isolation | PASS |
| Japanese IME composition / commit / send / language switch | PASS |

`assets/seed-san.vrm`はruntime成立probeにだけ使用した。公式`ene` assetのidentity、look、
material、silhouette、motion acceptanceを代替せず、#1651は未完了のままとする。

## Final status

**WINDOWS D/F ACCEPTANCE COMPLETE EXCEPT OFFICIAL ENE ASSET**

Windows Stage 7 completionに必要なWindows側の残件は#1651だけである。
