# Stage 7 D/F production implementation（実機 acceptance ではない）

実施日: 2026-09-21  
統合 base: `bef697813e9bb10642116d0f1c59a7900fc583f0`  
実装・自動gate SHA: `65625b5453e32ae5e83f759d0ddad05ce364cf1c`
環境: Ubuntu 26.04 x86-64、isolated X11 software-rendering smoke、Windows desktop なし。

## 結論

D/F の production candidate と測定経路を実装した。生成 VRM 1.0 fixture、Linux の自動 test、Windows x86-64 cross-check は実施した。一方、KDE Wayland compositor、Windows 11 desktop、Linux Secret Service、公式 `ene` VRM #1651 はこの環境に無い。したがって overlay / VRM の実機 probe、Performance Gate、Stage 7 / Milestone 1 は **未実施 / 未完了** のままである。

## D: Body

- `vrm-runtime` 0.1を `ene-body` だけに採用。strict VRM 1.0 load後に renderer primitive、expression、LookAt、SpringBoneを必須検証する。
- `PoseHint` だけをIPCし、humanoid rotation、expression weight、LookAt target、SpringBone state、CPU renderer frame dataはBody内で計算する。会話本文、秘密、Task command、関節角のIPC variantは無い。
- 約30 Hzのruntime update。Waylandは`wl_surface.frame`でpresentをpacingし、非表示時はruntime/presentを停止する。1 frameのBody-local処理が100 ms以上なら1秒presentを休止し、missed tickをcatch-upしない。
- wgpuのreal surfaceをFIFO/transparent alphaで構成し、CPU-baked meshのunlit fallbackを描画する。device lost / surface / out-of-memoryは`GpuFail`として親へ返し、Bodyだけを縮退させる。
- KDE Wayland: `zwlr_layer_shell_v1` overlay、keyboard interactivityなし、input region、drag/resize/hide、scale追従、`wp_presentation.feedback`。
- Windows: layered/DWM topmost tool popup、非矩形`WM_NCHITTEST` click-through、caption drag、bottom-right resize、DPI change、hide/restore。別foreground windowがmonitor全体を覆う間はpresentを止める。
- native backendを開けない場合はHeadlessへ落ちるが、`OverlayUnavailable`を必ず返し、production成功として扱わない。
- desktop supervisorは常時GPU skipを削除し、公式assetが存在する場合だけasset/placement/pose/showを投影する。Body failure後もdesktop/Hostは独立している。

### VRM probeの分離

自動testは再配布可能な生成GLB/VRM 1.0 fixtureでstrict load、humanoid、5 pose、expression、LookAt、SpringBone、有限なrenderer frame dataを確認する。`ene-body-asset-probe PATH.vrm`は同じruntime条件をJSONで出す。

これは公式`ene` asset acceptanceでも、透明surface表示の証拠でもない。公式assetは #1651 待ちである。

## F: measurement

- Linux `/proc/<pid>/stat` user/system と `VmRSS`、Windows `GetProcessTimes` と Working SetをPIDごとのraw pointとして保持する。
- 全対象PID合算のmachine CPU%、1-core equivalent、PID別0.5-core busy-wait、RSS mean/peakを計算する。Host/Desktop/Bodyの各1 PIDを必須にし、追加常駐processは`--other NAME:PID`で含める。
- Waylandはsurface commitごとのSubmittedと`wp_presentation` Presented/Discarded/Missingをcorrelation IDで記録する。未解決をMissingへ変換する。frame callbackやrender requestは分子にしない。
- WindowsはBody PID + swap-chainに限定したPresentMon CSVを読み、warmup後の固定区間だけを選ぶ。標準のms列と旧`TimeInSeconds`を区別し、display timingのない相関行をMissing、drop/`DisplayedTime=NA`行をDiscardedにする。`--presentmon-exe`指定時はBody PID、出力先、計測時間を固定してcapture processも起動・終了確認する。
- cancel操作はGUI input、Host outcome（intakeより遅い保守的上限）、Slint `AfterRendering`を同じprocess-local monotonic clockで記録する。DesktopはBody-only GPU境界を守るため明示的にSkia software rendererを選び、paint notifier非対応時はGUIを停止せずevidenceを生成しない（Performance GateはPass不可）。
- click-throughは実compositor下の窓へ入力が届いた外部raw evidenceを必須にする。自己申告なし、欠測、空evidence、1秒以上のblockはPassにならない。
- JSONとhuman reportは同じ`MeasurementRecord`から生成する。private verdictにより`evaluate()`以外からPassを構築できない。

計測時はdesktopを次のtrace環境変数付きで起動する。

```sh
ENE_PRESENTATION_TRACE_JSONL=/path/presentation.jsonl \
ENE_INTERACTION_TRACE_JSONL=/path/interaction.jsonl \
ene-desktop --gui ...
```

Wayland campaign例（release build、実PID・SHA・kernel、click-through JSONが必要）:

```sh
ene-measure \
  --host HOST_PID --desktop DESKTOP_PID --body BODY_PID \
  --sha EXACT_SHA --kernel KERNEL_BUILD --duration-secs 300 \
  --wayland-feedback-jsonl /path/presentation.jsonl \
  --interaction-jsonl /path/interaction.jsonl \
  --click-through-json /path/click-through.json \
  --environment-json /path/environment.json \
  --output-json /path/result.json --output-report /path/result.txt
```

WindowsではWayland引数の代わりに`--presentmon-exe PresentMon.exe --presentmon-csv TRACE --swap-chain ID`を使う。既に外部capture済みなら`--presentmon-exe`だけを省く。CSV/JSONを読み込んでも集計値は再計算する。debug binary、300秒未満、40桁SHAでない記録、環境JSONの欠測、Body PID不一致、discarded/missing、時刻/output相関不能はPassにならない。

環境JSONは次の全fieldを持つ。値は実機から採取し、空文字にしない。

```json
{
  "desktop_session": "KDE Plasma 6 Wayland",
  "cpu_model": "...",
  "gpu_model": "...",
  "gpu_driver": "...",
  "display": "2560x1440",
  "scale": "150%",
  "ui_language": "ja",
  "asset": "ene.vrm sha256:...",
  "render_backend": "Vulkan",
  "build_flags": "cargo build --release --locked"
}
```

click-through JSONは実 compositor 下の別surfaceが透明領域のclickを受けたraw logを先に保存し、そのlogのpathと観測結果を次の形で渡す。実surfaceを使わない自己申告JSONはacceptance証拠にしない。

```json
{
  "compositor": "KWin Wayland 6.x",
  "transparent_click_reached_underlying_window": true,
  "maximum_input_block_secs": 0.012,
  "raw_evidence": "/path/to/underlying-surface-input.jsonl"
}
```

## 実施済みの自動 gate

- `cargo fmt --all -- --check`
- `cargo build --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`（1547 pass / 71 suites）
- `cargo doc --workspace --no-deps`
- `cargo test -p ene-body --all-targets`（26 pass）
- `cargo test -p ene-desktop --all-targets -- --test-threads=1`（70 pass）
- `cargo test -p ene-core --test stage6_e2e -- --test-threads=1`（29 pass）
- `cargo check -p ene-body --target x86_64-pc-windows-gnu --all-targets`
- isolated X11で通常起動と`ENE_INTERACTION_TRACE_JSONL`指定起動の両方をsmokeし、Chat / Management window表示とpaint notifier登録エラーが無いことを確認

上記は同じ実装treeで成功した。Windows native CIはPRで別途確認する。

## 未実施 / blocker

1. 公式`ene` VRM 1.0 #1651のruntime probeと見た目 acceptance。
2. KDE Wayland実sessionのlayer-shell、透明、compositor click-through、drag/resize/hide/restore、HiDPI、presentation feedback。
3. Windows 11実機のlayered/DWM、透明hit-test、drag/resize/hide/restore、HiDPI、PresentMon相関。
4. release 5分の全PID CPU/RSS、固定区間の実presented FPS、cancel 1秒、click-throughを揃えたPerformance Gate。
5. Linux Secret Serviceと両OSの日本語IMEを含む既存acceptance残件。

上記をfake/headless/XWaylandでPassに置き換えない。
