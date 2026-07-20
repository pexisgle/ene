# プロアクティブ発話 / プロアクティブ判定仕様

Ene は、ユーザーのデスクトップ操作状態（アクティブウィンドウ、入力アイドル時間）、画面テキストの概要、および自身の保留タスク（約束など）を定期的にスキャンし、ユーザーからの指示プロンプトがない状態でも自発的に話しかける機能（プロアクティブ発話）を備えています。このドキュメントでは、決定論的なフィルタリングゲートと LLM による発話判定処理について定義します。

---

## 1. データ構造

### 1. ホスト側から受信する観察データ

#### `ActivitySnapshot` (パブリック / 構造体)
ユーザーのプライバシーに配慮した形でホストアプリが抽出したデスクトップの操作状態：
*   `idle_seconds: Option<u64>`: ユーザーが最後にキーボードやマウス操作を行ってからの経過秒数。
*   `active_window_label: String`: 正規化されたアプリケーション名（例: 単なるタイトルバー名ではなく `Browser` や `CodeEditor` などの大分類値）。
*   `recent_change: String`: アクティブなアプリケーションウィンドウが変更された際の変化イベント概要。

#### `ProactiveObservation` (パブリック / 構造体)
デスクトップのアクティビティをまとめて格納するフレーム構造体：
*   `captured_at_unix_ms: u64`: データがキャプチャされた時刻。
*   `activity: Option<ActivitySnapshot>`: 操作状態のスナップショット。
*   `screen_summary: Option<String>`: 画面内に映っているテキスト情報の簡潔な要約テキスト（生のスクリーンショットデータは保持されません）。

---

### 2. 制御状態パラメータ

#### `ProactiveSuppressionState` (パブリック / 構造体)
キャラクターが過剰に連発して話しかけてしまう（発話の氾濫）のを防止するための状態パラメータです：
*   `seconds_since_user_input: u64`: 最後にユーザーからチャット発言を受信してからの経過秒数。
*   `seconds_since_proactive: u64`: キャラクターが最後にプロアクティブ発話を行ってからの経過秒数。
*   `proactive_turns_this_session: usize`: 現在のセッション中に実行されたプロアクティブ発話の総回数。
*   `user_turn_busy: bool`: アクターがユーザーのターンの推論中、またはツール実行中であるかを示す状態値。

#### `ProactiveDecision` (パブリック / 構造体)
LLM が判定し、JSON 応答する構造化決定モデル：
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

## 2. 決定論的フィルタリングゲート (`gate.rs`)

高コストな LLM 判定を毎サイクル実行するのを避けるため、まず決定論的なルールゲート (`evaluate_deterministic_gates`) に通し、発話対象外の条件を排除します：

### ゲート却下コード (`GateRejectReason`)
以下のいずれかの条件に合致した場合、即座に発話判定をスキップします：
*   `UserTurnBusy`: アクターがターン処理を実行中、またはユーザーの入力を待機中。
*   `CooldownActive`: 前回のプロアクティブ発話からの経過秒数が `cooldown_seconds` 未満。
*   `SystemSessionLimitExceeded`: セッション内でのプロアクティブ発話回数が設定上限値に達している。
*   `NotIdle`: ユーザーの最終操作からの経過時間が短く、まだアイドル状態に達していない（`idle_seconds_required` 未満）。
*   `UserActiveWindowEmpty` / `ActiveWindowMuted`: 現在のアクティブウィンドウが空、またはブラックリスト（フルスクリーンでのゲームプレイやメディア再生中など）に登録されている。

---

## 3. アクション決定モデル処理 (`mod.rs`)

#### `parse` (for ProactiveObservation)
*   **シグネチャ**: `fn parse(raw: Option<&str>) -> Self`
*   **説明**: ホストプロセスから送られてきた操作情報の JSON 文字列をパースしてデータ構造に復元します。

#### `silent` (for ProactiveDecisionResult)
*   **シグネチャ**: `pub fn silent(reason: impl Into<String>) -> Self`
*   **説明**: 話しかけない（沈黙を維持する）判定結果を生成し、その具体的な却下理由を記録します。

#### `allows_generation`
*   **シグネチャ**: `pub fn allows_generation(&self, min_confidence: f64) -> bool`
*   **説明**: LLM の発話決定フラグ `should_speak` が真であり、かつ判定の確証度が設定値 `min_confidence` を上回っているかを再確認します。

---

## 4. プロンプトアセンブリ構築 (`prompt.rs`)

#### `build_decision_messages`
*   **シグネチャ**: `pub fn build_decision_messages(context: &ProactiveContext, prompt_language: &str) -> Vec<LlmMessage>`
*   **プロセス**:
    1.  感情パラメータ、アクティブな約束、過去の会話履歴などの文脈をシステム指示として構成します。
    2.  ユーザーの最新のデスクトップウィンドウ状態および画面のテキスト情報をプロンプトにインジェクトします。
    3.  これらを LLM 判定用のプロンプトメッセージ配列としてまとめます。

#### `format_context_block`
*   **シグネチャ**: `fn format_context_block(context: &ProactiveContext) -> String`
*   **説明**: アクティブウィンドウ分類名、経過アイドル時間、および画面テキスト要約を、プロンプト用のフォーマットされた構造テキストに展開します。

---

## 5. 決定 JSON データの解決と検証 (`parse.rs`)

#### `decision_schema_object`
*   **Signature**: `pub fn decision_schema_object() -> Value`
*   **Description**: LLM 応答用のスキーマオブジェクト定義を構築します。

#### `decision_schema`
*   **Signature**: `pub fn decision_schema() -> Value`
*   **Description**: JSON スキーマをカプセル化したルート構造体オブジェクトを定義します。

#### `parse_decision_json`
*   **シグネチャ**: `pub fn parse_decision_json(raw: &str) -> ProactiveDecision`
*   **説明**: LLM から返された応答テキストを解析し、マークダウンのコードブロック指示子などのノイズを排除した上で、`ProactiveDecision` にデシリアライズします。パースエラーが発生した場合は自動的に「沈黙（話しかけない）」デフォルト値にフォールバックします。

#### `extract_json_object`
*   **Signature**: `fn extract_json_object(raw: &str) -> Option<&str>`
*   **Description**: 応答テキストの中から JSON の括弧 `{}` を探索し、内側のデータのみを抽出します。
