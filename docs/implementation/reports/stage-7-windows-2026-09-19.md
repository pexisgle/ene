# Stage 7 Windows 実機検証

実施日: 2026-09-19 (Asia/Tokyo)
対象 PR: https://github.com/pexisgle/ene/pull/1657
対象 commit: `19084ee223511db88d80bc61bbbd2ce497b76d9e`
環境: Windows 11 Pro x86-64 / 10.0.26200、rustc 1.98.1、debug build、Slint 1.18.0。

## 結論

**Windows の最終 acceptance は未合格。** 自動テスト 89 件は成功したが、製品 GUI の再起動で接続を回復できない。セットアップ復帰と日本語表示にも不足がある。加えて、ユーザーから「UI 全体のデザイン・見た目が製品として不十分」と指摘があり、実画面で確認したレイアウト・情報階層・表示内容の問題を下記に記録する。OS credential store / 実 overlay / VRM / 性能 gate は今回合格としていない。

## 対象と方法

PR #1657 は B/C1/C2/C3/D/E の統合 branch。main には実装がないため、上記 commit から専用 worktree を作成した。

- 検証専用データ: 検証 worktree 内の `target/stage7-windows-evidence/data`。既存ユーザーデータから分離した。
- 起動 stdout/stderr: 同じ evidence directory 内の `desktop*.log`
- Windows UI 操作: computer-use の `@oai/sky`。各操作後の画面をこのタスクのツール結果に記録。スクリーンショットの別ファイル保存はしていない。
- 初回ペアリングの最終確認はユーザー本人が実施。Computer Use では代行していない。
- 実 API キー登録、外部 provider 会話、実データ削除は実施していない。
- この報告コミットは実装を変更しない。検証用 GUI と、この検証が起動した Host は終了済み。ローカルのテストデータ・ログはコミット対象外。

## 自動検証

| コマンド | 結果 |
|---|---|
| `cargo build --locked -p ene-core -p ene-desktop -p ene-body` | 成功。Windows linker stdout に由来する linker_messages warning 1 件あり |
| `cargo test --locked -p ene-desktop -p ene-body --all-targets -- --test-threads=1` | 79 passed、10 suites |
| `cargo test --locked -p ene-core --test stage7_a1 -- --test-threads=1` | 10 passed、1 suite |

全コマンドは RTK 経由で実行した。A1 の初回実行は、実機確認用の ene-core.exe が稼働中で Cargo が exe を置換できず、Windows の os error 5 で中断した。検証用 GUI / Host を終了して同じコマンドを再実行し、10 件成功した。これは assertion failure ではない。

PR の Check / Check Windows も確認時点では pass だった。実 GUI 操作の不具合は、自動テスト成功とは分けて扱う。

## 実画面で確認したこと

| 項目 | 結果と限界 |
|---|---|
| 新規データから起動 | GUI と Host が起動。初回ペアリングの確認画面は操作説明なしで UUID だけを表示 |
| 初回ペアリング | ユーザー本人の確認操作後、接続済み・言語選択画面へ遷移 |
| セットアップ案内 | 言語 → 同梱 ene → クラウド送信・費用 → API key 入力欄を表示。登録完了までは未検証 |
| 日英切り替え | ナビゲーションと案内が切り替わる。未送信の日本語・英数字本文を保持 |
| 日本語入力 | `Windows確認テスト 日本語 ABC 123` の入力・表示を確認 |
| 実 IME | 入力欄で Alt+grave により日本語 IME に切り替え、物理 a キーから未確定「あ」と候補を表示。Enter 1 回で「あ」が確定し、送信されない。単一の基本ケースであり、全 IME 経路の合格ではない。入力モードは元に戻した |
| セットアップ前の送信 | 上記テスト文は受付拒否。英語の内部エラー `intake was not accepted: RoundIntakeOutcome` を日本語 UI に表示。返答成功とは表示されなかった |
| 各ページ | 会話、履歴、記憶、タスク、設定、情報、利用量、削除の画面が開く。履歴/記憶/タスクはデータのない状態。実 provider 由来の内容の検証ではない |
| AboutSlint | Slint ロゴ・バージョン 1.18.0 を確認 |
| avatar 不在の画面操作 | テキスト画面と管理画面の切り替えができる。実 renderer crash の代用にはしない |
| GUI close | GUI PID 25344 を閉じた後も Host PID 31236 が残る |
| GUI restart | **失敗**。同じデータで GUI PID 10896 を起動すると「接続中・unknown」のまま。後続の再観測でも回復せず、Host は同じ PID 31236 で稼働 |

## 再現した不具合

### 1. 登録済み GUI の再起動で接続を回復できない (優先度 P1)

手順:

1. 新規データで GUI を起動する。
2. 初回ペアリングを本人が確認し、「接続済み」を確認する。
3. GUI を閉じる。Host はそのまま残す。
4. 同じデータで GUI を起動し直す。

実際: 言語選択画面に戻り、「接続中・unknown」のまま。接続エラーや回復手段は出ない。
期待: 登録済み Client として接続を回復し、Host の現在状態を提示する。

原因を裏付けるコード:

- [main.rs:64](https://github.com/pexisgle/ene/blob/19084ee223511db88d80bc61bbbd2ce497b76d9e/apps/ene-desktop/src/main.rs#L64): 起動時に常に `begin_pairing()` を呼び、失敗を無視する。
- [session.rs:99](https://github.com/pexisgle/ene/blob/19084ee223511db88d80bc61bbbd2ce497b76d9e/apps/ene-desktop/src/session.rs#L99): `connect_until_pending()` が接続成功を `pairing was expected to pend` エラーにして、接続した Client を保持しない。

### 2. セットアップから他ページへ移動すると GUI 内で復帰できない (優先度 P1)

手順:

1. セットアップで API key 入力の段階まで進む。
2. 上部の会話を開く。
3. 設定を開いてセットアップの続きを探す。

実際: 設定ページは「OpenAI API キーを登録します（この欄は会話ではありません）」という説明文だけで、キー入力欄、モデル選択、セットアップに戻る操作がない。上部ナビゲーションにも Wizard の入口がない。
期待: 未完了のセットアップに戻り、同意・登録・モデル選択を案内に従って完了できる。

コード:

- [app.slint:171](https://github.com/pexisgle/ene/blob/19084ee223511db88d80bc61bbbd2ce497b76d9e/apps/ene-desktop-ui/ui/app.slint#L171): Settings は body の Text のみ。
- [main.rs:616](https://github.com/pexisgle/ene/blob/19084ee223511db88d80bc61bbbd2ce497b76d9e/apps/ene-desktop/src/main.rs#L616): settings を含む body に wizard_body を入れている。

### 3. 日本語設定でも操作・拒否理由が未翻訳 (優先度 P2)

実際:

- タスクの Refresh / Select / Resume / Workspace と入力欄の説明が英語。
- タスク詳細は `no task selected`、`presentation not-presented` 等。
- 送信拒否理由は `intake was not accepted: RoundIntakeOutcome`。
- 利用量・削除画面も内部名称や英語の説明をそのまま表示。

期待: 日本語のラベル・拒否理由・状態説明。
根拠例: [app.slint:163](https://github.com/pexisgle/ene/blob/19084ee223511db88d80bc61bbbd2ce497b76d9e/apps/ene-desktop-ui/ui/app.slint#L163) の固定英語ボタン。

### 4. UI 全体のデザイン品質が製品として不十分（ユーザー指摘・リリース前に要改善）

ユーザー評価: UI 全体の見た目が優れておらず、現状のデザインでは受け入れられない。これは一部のラベルや配色だけの問題ではなく、画面全体の構成と視覚的な完成度への指摘である。

今回の実画面で確認した具体例:

| 観点 | 観察した問題 | 利用者への影響 |
|---|---|---|
| 画面構成・余白 | 初回セットアップでは上部の 10 個のナビゲーションが縦に大きく引き伸ばされ、説明文・状態・操作の間に広い空白がある。会話画面へ移るとナビゲーションの高さが大きく変わる | 説明と次の操作が離れ、画面ごとに配置の一貫性がない |
| 情報の優先順位 | ページを切り替えても大見出しは常に `ene`。現在ページ、作業の目的、主要操作の視覚的な区別が弱い | どこにいて何をする画面なのか把握しにくい |
| ナビゲーションと操作 | 会話・各管理機能・削除・言語切り替えが上部の同列ボタンとして並ぶ。主要操作と補助操作の強弱が乏しい | 日常操作と管理操作の関係や、最初に選ぶ操作が伝わらない |
| 管理画面の情報整理 | タスク・利用量・削除を、内部名称を含むプレーンテキストの塊で表示している。例: `presentation not-presented`、`cap usage-cap-system-daily_utc-none system daily_utc no-cap`、`mark deletion-view:0/-/0/-` | 項目、値、状態、次の操作が読み取りにくく、デバッグ表示のように見える |
| 確認・空状態・エラー | 初回確認は UUID だけ、記憶の空状態は `(none)`、送信拒否は内部エラー名。説明と回復手順が不足する | 何を確認するのか、なぜ空なのか、どう先へ進むのか判断しにくい |
| 視覚的なバランス | 小さな本文が広い領域の中に孤立する一方、情報画面では Slint ロゴが画面の大部分を占める | 内容の重要度と表示面積が釣り合わず、製品全体のまとまりがない |

改善時には、既存機能の情報設計・ページ見出し・ナビゲーション・余白・文字サイズ・主要操作・空状態・エラー表示を一体として見直す。必須の AboutSlint 表示は維持する。色の変更だけでこの指摘を解消したことにはしない。

この評価は、新しい製品機能や未合意の数値基準を acceptance に追加するものではない。既存機能の提示品質へのユーザー指摘として、機能テストの pass と独立に記録する。再評価は、日本語・英語の実画面で、セットアップ・会話・管理画面を一通り操作して行う。この報告時点では UI 改善も再評価も未実施。

## 未検証・実装不足を分けて記録

- **製品 OS credential store:** コード上、Host の通常 open は EnvCredentialStore を使用し、serving-time put は fail-closed。MemoryCredentialStore を使う統合テストの成功で、製品 GUI のキー登録成功を証明していない。[serve.rs:676](https://github.com/pexisgle/ene/blob/19084ee223511db88d80bc61bbbd2ce497b76d9e/apps/ene-core/src/serve.rs#L676)
- **モデル選択と実 provider 会話:** 実 API キーを登録していないため未検証。会話、Memory 形成、Task 実行の実画面 E2E は未完了。
- **Body / VRM:** Windows DWM は NotRun を返す stub、Overlay::open は Headless。公式 ene VRM 未収録も PR が明記。透過表示・移動・resize・表情・SpringBone は未検証。[dwm.rs:13](https://github.com/pexisgle/ene/blob/19084ee223511db88d80bc61bbbd2ce497b76d9e/apps/ene-body/src/window/dwm.rs#L13)、[window/mod.rs:34](https://github.com/pexisgle/ene/blob/19084ee223511db88d80bc61bbbd2ce497b76d9e/apps/ene-body/src/window/mod.rs#L34)
- **削除・上限更新・秘密入力:** 管理画面を開くことだけを確認し、実画面の高権限確定操作や秘密値入力はしていない。関連自動テストの成功と区別する。
- **性能:** Host + desktop + Body の通常表示状態が成立していないため、5 分 CPU / resident / 実表示 FPS / 1 秒受付の正式測定を実施していない。
- **CI と実機の差:** 統合テストは DesktopRuntime を直接操作する。今回再現した製品 main() の起動・再起動や実画面ナビゲーションの成立までは保証していない。
