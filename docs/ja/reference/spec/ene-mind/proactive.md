# Proactive Speech / 能動話話判断仕様

Ene は、ユーザーからの明示的なメッセージ入力を待つだけでなく、ユーザーのOSアクティビティ（アクティブウィンドウ、待機時間）や画面内容、自身の未完了タスク（コミットメント）を監視し、能動的（Proactive）に話しかける機能を備えています。本ドキュメントでは、能動発話の判定ゲートおよび LLM 意思決定パイプラインの仕様を定義します。

---

## 1. データ構造

### 1. ホスト観測情報 (Observations)

#### `ActivitySnapshot` (公開 / 構造体)
デスクトップ側（ホストアプリ）で収集された、プライバシーに配慮された操作状況情報。
*   `idle_seconds: Option<u64>`: マウス/キーボード入力が途絶えてからの秒数。
*   `active_window_label: String`: 現在アクティブなアプリ名（生のウィンドウタイトルは含みません。例: `Browser`, `CodeEditor` 等）。
*   `recent_change: String`: アクティブアプリが切り替わった際の変化内容。

#### `ProactiveObservation` (公開 / 構造体)
デスクトップから一定周期で脳にプッシュされる観測全体。
*   `captured_at_unix_ms: u64`: 観測タイムスタンプ。
*   `activity: Option<ActivitySnapshot>`: OSのアクティビティ状況。
*   `screen_summary: Option<String>`: スクリーン上に映るテキスト情報の要約（画像データ自体は含みません）。

---

### 2. 抑制・制御パラメータ

#### `ProactiveSuppressionState` (公開 / 構造体)
頻繁な話しかけによるユーザー体験の悪化（スパム化）を防ぐための、時間的抑制状態。
*   `seconds_since_user_input: u64`: 最後のユーザー発言からの経過秒数。
*   `seconds_since_proactive: u64`: 最後のマスコットからの能動発話からの経過秒数。
*   `proactive_turns_this_session: usize`: 本セッション中に能動発話した回数。
*   `user_turn_busy: bool`: 現在、ユーザーの入力処理中（アクター実行中、または承認・追加入力待ち状態）かどうか。

#### `ProactiveDecision` (公開 / 意思決定構造体)
軽量LLMが返却する、話しかけるかどうかの構造化判断結果。
```rust
pub struct ProactiveDecision {
    pub should_speak: bool,
    pub confidence: f64,
    pub screen_digest: String,
    pub reason: String,
    pub topic_hint: String,
    pub urgency: ProactiveUrgency,
}
```

---

## 2. 決定論的ゲートフィルタ (`gate.rs`)

LLM を用いた判定は計算コストが高いため、まず決定論的なルールゲート (`evaluate_deterministic_gates`) にて不要な発話を即座に弾きます。

### ゲート却下理由 (`GateRejectReason`)
以下の条件に引っかかった場合、LLMへの問い合わせを行わずに却下（`ProactiveDecisionOutcome::Skipped`）します。
*   `UserTurnBusy`: ターン処理がすでにビジー（ユーザー応答待ちやLLM実行中）。
*   `CooldownActive`: 前回の能動発話から `cooldown_seconds`（最小インターバル）が経過していない。
*   `SystemSessionLimitExceeded`: セッション内での能動発話上限回数を超えている。
*   `NotIdle`: ユーザーの操作が活発（`idle_seconds_required` 未満）。
*   `UserActiveWindowEmpty` / `ActiveWindowMuted`: アクティブなウィンドウ情報が空、または「ゲーム中」「プレゼン中」などマスコットが話しかけてはいけない非表示ブラックリストアプリに合致する。

---

## 3. LLM 意思決定パイプライン (`decide_proactive_speech`)

ゲートをクリアした場合のみ、以下の流れで LLM 判定を実行します。

1.  **プロンプト構築 (`build_decision_messages`)**:
    -   現在の感情状態（PAD値）、未完了タスク（コミットメント）、直近の会話履歴、アクティブアプリ、スクリーン要約テキストをシステムプロンプトに統合。
    -   マスコットの性格に合わせ、「今話しかけるべき話題や約束、突発イベントがあるか」を客観的に検証するよう指示。
2.  **スキーマ制約付きLLM実行**:
    -   `decision_schema()` を用いて、LLMに対して `ProactiveDecision` の JSON スキーマでの応答を強制。
    -   意図しないフォーマット崩れや自由テキスト出力を防止します。
3.  **確信度判定**:
    -   LLMが返した `should_speak` が `true` かつ、`confidence` が設定ファイルで規定された `min_confidence` を超えている場合のみ、話しかけ（`EneCommand::Run`）をトリガーします。
    -   判定に成功した場合、`topic_hint` に指定された話題・指示に沿って会話ターンが生成されます。
