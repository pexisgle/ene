# Stage 7 Linux 検証（Cloud Agent VM）

実施日: 2026-09-19 (UTC)
対象 PR: https://github.com/pexisgle/ene/pull/1657 （E 統合。本報告の実装修正は stacked `cursor/stage7-f-linux-0fdf`）
環境: Ubuntu 24.04.4 LTS x86-64、kernel 6.12.94+、`DISPLAY=:1` Xtigervnc 1920x1200 + xfce4-session / xfwm4。`WAYLAND_DISPLAY` なし。logical CPUs 4。rustc/cargo 1.98.1。debug build。`direnv` / `rtk` なし、`cargo` 直実行。

## 結論

**この Cloud Agent の Linux（Ubuntu 24.04.4 LTS、X11 XFCE）を、本マイルストーンの Linux 検証として記録する。NixOS 26.11 公式 desktop を待たない（検証対象の読み替え）。26.11 合格とは書かない。**

この VM で Stage 7 の自動テスト（A1 / B / C1 / C2 / C3 / E、および Stage 6 e2e）は成功した。X11 XFCE 上で製品 `ene-desktop` を操作し、ウィザード事実、ページ切替、日英切替、Body 殺害後の chat 入力面を確認した。

KDE Wayland ではない。overlay / IME / layer-shell / 透過 Body / 公式 ene VRM は **未実施**。headless CI と X11 成功を KDE Wayland overlay 合格にしない。Performance Gate と Windows 11 は別途残る。Stage 7 / Milestone 1 は完了としない。

実画面で再現した GUI の不具合（stale snapshot、stretched nav、`CredentialRefused` の誤表示）は F branch で直した。Windows 報告の P1（再起動で接続回復しない、Wizard 復帰不能）と UI 品質指摘は、この Linux 修正の対象外として残る。

## SHA

| 対象 | SHA |
|---|---|
| 検証開始時の E tip（body / A1 / stage6_e2e / workspace clippy） | `19084ee223511db88d80bc61bbbd2ce497b76d9e` |
| Linux GUI 修正後（desktop 再テスト・X11 walkthrough） | `22aeabf9eee53da73d5e955c76767f9640e0344b` |
| E の Windows 報告を merge した F tip | `fbb62b0bcebabed8248dd70ea62d600ed2b4c2e4` （この報告コミットの親） |

作業 tree: `/home/ubuntu/ene-e`。workspace root `/workspace` は `cursor/stage6-land-c0df` のまま触っていない。

## 自動検証

コマンドは実際にこの VM で実行した。CI の Check / Check Windows 成功を代用していない。

| コマンド | SHA | 結果 |
|---|---|---|
| `cargo test --locked -p ene-desktop --all-targets -- --test-threads=1` | 初回 `19084ee2` | 1 fail: `killing_body_leaves_chat_settings_and_cancel_alive`。原因は製品 assertion ではなく、`CARGO_TARGET_DIR=/tmp/ene-e-ci-target` に `ene-body` が未ビルド |
| 同上（`ene-body` を先に build） | `19084ee2` 再実行、および `22aeabf9` | 55 passed（lib 29 / stage7_b 6 / c1 3 / c2 5 / c3 5 / e 7） |
| `cargo test --locked -p ene-body --all-targets` | `19084ee2` | 30 passed |
| `cargo test --locked -p ene-core --test stage7_a1 -- --test-threads=1` | `19084ee2` | 10 passed |
| `cargo test --locked -p ene-core --test stage6_e2e -- --test-threads=1` | `19084ee2` | 29 passed、387s。全件実行。間引きなし |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | `19084ee2`、F tip でも再実行 | `-D warnings` 成功 |
| `cargo fmt --all -- --check` | `19084ee2` と `22aeabf9` | 成功 |

ログ: `/opt/cursor/artifacts/cargo_test_ene_desktop*.log`、`cargo_test_ene_body.log`、`cargo_test_stage7_a1.log`、`cargo_test_stage6_e2e.log`、`cargo_clippy_workspace.log`、`cargo_clippy_fmt_f_linux.log`、`fmt_check.log`。

## Slice 判定（この Linux VM）

| Slice | 自動 | 実 GUI / probe | 判定 |
|---|---|---|---|
| A1 | stage7_a1 10 pass | 製品 GUI の control 確認面はウィザードの「確認する」まで | 自動 pass。実 OS store / 本人確認の最終 acceptance ではない |
| B | stage7_b 6 pass | X11 で言語・同梱 ene・費用・API key 欄・日英・会話欄 | 自動 pass。実 provider 会話と IME composition は未実施 |
| C1 | stage7_c1 3 pass | Memory ページを開いた（空 `(none)`） | 自動 pass。実 Memory 形成の GUI E2E は未実施 |
| C2 | stage7_c2 5 pass | Tasks ページを開いた（empty / 英語内部状態） | 自動 pass。実 Workspace Task E2E は未実施 |
| C3 | stage7_c3 5 pass | Usage / Deletion ページを開いた | 自動 pass。実削除確定・cap 更新の GUI 操作は未実施 |
| D | ene-body 30 pass（headless / GpuFail） | X11 で独立 `ene-body` を起動し kill。overlay Headless、GPU `NoAdapter` | **overlay / IME / layer-shell / 公式 VRM は未実施**。X11 成功を KDE Wayland にしない |
| E | stage7_e 7 pass（Body kill 含む） | Body 不在でも chat 入力欄が生き、送信ボタンを押せる | 自動 pass。実 renderer crash の代用にはしない |
| F | measure.rs skeleton は `Unmeasured`、`Pass` を構築しない | 下記 non-gate 30s サンプルのみ | **Performance Gate 未実施** |

## 実画面（X11 XFCE。KDE Wayland ではない）

操作は `xdotool` + 画面録画。Computer Use サブエージェントは使っていない。

| 項目 | 結果と限界 |
|---|---|
| 新規データから起動 | GUI と Host が起動。初回確認は UUID と「確認する / キャンセル」 |
| ウィザード事実 | 言語選択 → 同梱キャラ説明 → クラウド送信・費用 → API key 入力 |
| dummy key | `sk-linux-verify-not-a-real-key` を入力。製品 `EnvCredentialStore` が `CredentialRefused`。OS 保護ストアは使えない。成功とは表示されない |
| モデル割り当て | 未完了のまま（store 拒否）。会話は「内容を確認できません。条件を見直してください。」 |
| 日英切り替え | ナビと案内が切り替わる（Chat/History/… と 会話/履歴/…） |
| 各ページ | 会話・履歴・記憶・タスク・設定・情報・利用量・削除が開く |
| chat 入力 | セットアップ未完了のため送信は拒否。入力欄と送信ボタンは操作できる |
| Body kill | 監督外で `ene-body` を起動して kill。desktop / Host は生存。chat 入力が残る |
| 日本語 IME | **未実施**（xdotool の ASCII type のみ） |
| overlay / layer-shell / 透過 / VRM | **未実施**。Body は Headless + `GpuFail(NoAdapter)` |
| GUI 再起動からの接続回復 | この Linux セッションでは未実施（Windows 報告の P1） |

## この VM で直した不具合

1. **50ms tick が click 前の snapshot でページ/言語を巻き戻す。** `SnapshotPump` で新しい stamp だけを Slint スレッドに適用する。
2. **`CredentialRefused` を unexpected control answer と表示していた。** ドメイン結果として区別し、ウィザードへ戻す。
3. **ウィザードがナビ行を縦に引き伸ばし、Chat/Memory/JA/EN の hit target がずれる。** ナビを 32px 固定行にし、ページ本体を stretch + clip する。

Windows 報告 §「画面構成・余白」の stretched nav は、Linux X11 ではこの 3 番で再現し、F で直した。Windows 再評価はしていない。

## 性能（non-gate。合格と書かない）

first-party-desktop 第8節の方法（5 分 idle、release、presented FPS、通常 avatar 表示）は満たしていない。debug build。Body は Headless + GPU skip。`measure.rs` の verdict は `Unmeasured`。Linux 検証の記録環境は本 VM であり、NixOS 26.11 上の測定ではない。

30 秒 `/proc` サンプル（Host + desktop + Body、全 PID、Body 除外なし）:

| 項目 | 値 |
|---|---|
| wall | 30.000 s |
| logical CPUs | 4 |
| Σ cpu_seconds | 0.160 |
| machine % `100 * Σ / (wall * cpus)` | 0.133 |
| 1-core equivalent `Σ / wall` | 0.0053 |
| RSS peak（3 process） | 86417408 bytes（約 82.4 MiB） |
| FPS | 未実施（wp_presentation / PresentMon なし。X11） |

生値: `/opt/cursor/artifacts/perf_idle_nongate.json`。skeleton: `/opt/cursor/artifacts/measure_rs_idle_template.json`。

## 未実施のまま残すもの

- KDE Wayland overlay / IME / layer-shell / click-through（本 Linux 検証は X11 XFCE）
- 公式同梱 `ene` VRM（#1651）
- Windows 11 最終 acceptance（別報告。この VM ではない）
- 製品 OS credential store の put / read-back
- 実 provider 会話、Memory 形成、Task 実行、削除確定の GUI E2E
- Performance Gate（第8節の測り方。下記 30s サンプルは非ゲート）
- Stage 7 complete / Milestone 1 / Stage 8

Linux 側の自動テストと X11 製品 GUI 操作は本報告が証拠である。`PROGRESS.md` は Linux 検証済みと残件だけを短く指す。
