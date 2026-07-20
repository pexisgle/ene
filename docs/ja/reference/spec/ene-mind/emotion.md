# `EmotionEngine` / PAD 感情状態仕様

このドキュメントでは、Ene の感情状態モデル仕様について定義します。PAD（Pleasure-Arousal-Dominance：快・覚醒・支配）空間指標モデル、ルールベースによる即時感情アプレザル、時間経過に伴う感情自然減衰、および非同期 LLM 感情分類ループが含まれます。

---

## 1. 感情状態空間モデル (PAD 空間)

Ene はアクターの感情パラメータを以下の連続変数を用いて表現し、SQLite の `affect_states` テーブルに永続化します：

*   **Valence (快・不快) [-1.0 ..= 1.0]**: 喜び（正の値）と悲しみ（負の値）。
*   **Arousal (覚醒) [-1.0 ..= 1.0]**: 興奮（正の値）と沈静（負の値）。
*   **Irritation (苛立ち) [0.0 ..= 1.0]**: ユーザーの対応によって蓄積するストレス値。
*   **Affinity (親愛) [-1.0 ..= 1.0]**: ユーザーに対する信頼感。
*   **Fatigue (疲労) [0.0 ..= 1.0]**: 連続で発話した際の疲労指数。

---

## 2. ターン感情更新処理 (`EmotionEngine` コアメソッド)

#### `update_turn`
*   **シグネチャ**: `pub fn update_turn(&self, config: &EmotionConfig, input: &mut TurnAffectInput<'_>) -> AffectUpdateResult`
*   **プロセス**:
    1.  `apply_decay` を実行し、前回の感情更新からの経過時間（`elapsed`）に応じて感情パラメータを自然な基調値（0.0）にゆっくりと減衰（Decay）させます。
    2.  `apply_appraisal` を呼び出して、ユーザーの発話内のテキスト情報に基づくルールベース感情変化を計算・適用します。
    3.  非同期の LLM 感情分類結果の提案（`AffectProposal`）が存在する場合は、`merge_classifier_proposal` に送り、提案された感情に近づくようにブレンドマージします。
    4.  最終的な感情値が PAD モデルの指定限界値を超えないように安全クランプ（Clamping）します。
    5.  `compute_mood_label` を実行して、更新された感情値の PAD 空間座標に一致する気分（Mood）ラベルを再計算して返します。

#### `merge_classifier_proposal`
*   **シグネチャ**: `fn merge_classifier_proposal(state: &mut AffectState, proposal: &AffectProposal, min_confidence: f32) -> Option<AffectUpdateReason>`
*   **説明**: LLM 分析器が提案した感情（Valence, Arousal, Irritation, Affinity）を、信頼度（Confidence）が基準閾値 `min_confidence` を満たしている場合のみ、アクティブな感情状態に反映します。

#### `apply_weighted_blend`
*   **シグネチャ**: `fn apply_weighted_blend(state: &mut AffectState, field: &'static str, target: f32, weight: f32, deltas: &mut Vec<AffectDelta>)`
*   **説明**: 以下の線形補間数式を用いて、元の感情値を LLM の感情提案値に向けて適度にブレンド移動させます：
    $$X_{\text{new}} = (X_{\text{target}} - X_{\text{current}}) \times w + X_{\text{current}}$$
    （ここで $w$ は、提案信頼度スコアをブレンド比率として適用した重み係数です）。

#### `compute_mood_label`
*   **シグネチャ**: `pub fn compute_mood_label(state: &AffectState) -> String`
*   **説明**: PAD の座標特性を基に、以下のいずれかの代表気分（Mood）ラベルを決定します：
    *   `Joyful` (陽気): Valence > 0.3 かつ Arousal > 0.0
    *   `Relaxed` (のんびり): Valence > 0.2 かつ Arousal <= 0.0
    *   `Anxious` (不安): Valence < -0.2 かつ Arousal > 0.2
    *   `Depressed` (沈うつ): Valence < -0.2 かつ Arousal <= 0.2
    *   `Hostile` (敵対的): Irritation > 0.5
    *   `Neutral` (中立): いずれの閾値にも達しない場合の基準ラベル。

---

## 3. 時間経過による自然減衰モデル (`decay.rs`)

#### `apply_decay`
*   **シグネチャ**: `pub fn apply_decay(state: &mut AffectState, half_life_minutes: f64, elapsed: Duration) -> Option<AffectUpdateReason>`
*   **説明**: 喜び、興奮、苛立ち、および疲労の偏差パラメータを、半減期設定と経過時間（`elapsed`）に基づいて以下の数式で中立状態に近づけます：
    $$V_{\text{new}} = V_{\text{old}} \times e^{-\lambda t}$$
    （ここで $\lambda = \frac{\ln(2)}{\text{decay\_half\_life\_minutes}}$ です。親愛度パラメータは自然減衰の対象から除外されます）。

---

## 4. ルールベース感情アプレザル (`appraisal.rs`)

ユーザーのテキストパターンを判定し、一時的な感情のスパイクや蓄積を計算します。

#### `apply_appraisal`
*   **シグネチャ**: `pub fn apply_appraisal(state: &mut AffectState, user_message: &str, recent_turn_count: usize) -> Vec<AffectUpdateReason>`
*   **プロセス**:
    1.  `ascii_tokens` を使用してユーザー発話をワード配列に正規化します。
    2.  発話の文字数が非常に長い場合、または感嘆符が連続している場合（「！！」）は、アクターの覚醒度（Arousal）を即座に引き上げます。
    3.  会話の継続ターン数に応じて、段階的に疲労度（Fatigue）を加算します。
    4.  感謝を示す単語（「ありがとう」など）が含まれている場合は、快（Valence）と親愛（Affinity）のスコアをプラス加算します。
    5.  不快な単語や乱暴な表現を検出した場合は、苛立ち（Irritation）を増やし、快スコアをマイナス加算します。

#### `ascii_tokens`
*   **Signature**: `fn ascii_tokens(text: &str) -> Vec<String>`
*   **Description**: 特殊記号や空白を正規化し、検索用の英小文字ワードトークン配列を構築します。

#### `pattern_matches`
*   **Signature**: `fn pattern_matches(normalized: &str, pattern: &str) -> bool`
*   **Description**: 指定された文字パターンがトークン配列内に合致するか検証します。

#### `apply_field_delta`
*   **Signature**: `fn apply_field_delta(state: &mut AffectState, field: &'static str, delta: f32, deltas: &mut Vec<AffectDelta>)`
*   **Description**: 指定された特定の感情指標にデルタ（変化量）を加算し、設定可能な上限および下限の範囲内に安全クランプします。

---

## 5. ポストターン非同期 LLM 分類器 (`classifier.rs`)

皮肉や複雑なニュアンスの分析は、チャットが完了した後のバックグラウンド処理で LLM プロバイダを用いて実行されます。

#### `classify_for_config`
*   **シグネチャ**: `pub async fn classify_for_config(config: &ene_config::EneConfig, model_override: Option<&str>, max_tokens: u32, context: &ClassifierContext, timeout_secs: u64, lang: &str) -> Result<AffectProposal, CognitionError>`
*   **説明**: クライアントから転送された対話コンテキストを読み込んでプロンプトメッセージを構成し、感情分類判定を実行して、提案データ（`AffectProposal`）を作成します。

#### `classify_failure_reason`
*   **Signature**: `pub const fn classify_failure_reason(error: &CognitionError) -> &'static str`
*   **Description**: 感情分類処理中の例外エラーログ文字列をマッピングします。

#### `proposal_json_schema`
*   **Signature**: `fn proposal_json_schema() -> serde_json::Value`
*   **Description**: LLM に出力させる感情分類結果 JSON の JSON Schema を構築します。

#### `classify_with_resilient_fallback`
*   **Signature**: `async fn classify_with_resilient_fallback<F>(mut provider_factory: F, current_affect: &str, conversation: &str, timeout_secs: u64, lang: &str) -> Result<AffectProposal, ClassifierError> where F: FnMut(Option<u32>) -> Result<Box<dyn LlmProvider>, ClassifierError>`
*   **Description**: メインの LLM プロバイダ接続がタイムアウトや制限に達した場合に、速やかに代替のプロバイダを起動して感情分類をリトライ実行します。

#### `classify_with_timeout`
*   **Signature**: `async fn classify_with_timeout(provider: &dyn LlmProvider, current_affect: &str, conversation: &str, timeout_secs: u64, lang: &str, transport: ClassifierTransport, json_schema: &serde_json::Value) -> Result<AffectProposal, ClassifierError>`
*   **Description**: タイムアウト制約の中で LLM を呼び出します。

#### `call_provider`
*   **Signature**: `async fn call_provider(provider: &dyn LlmProvider, messages: &[LlmMessage], transport: ClassifierTransport, json_schema: &serde_json::Value) -> Result<String, ClassifierError>`
*   **Description**: LLM のテキストまたは JSON-RPC 送信プロトコルを実行し、レスポンスを取得します。

#### `strip_markdown_fences`
*   **Signature**: `fn strip_markdown_fences(raw: &str) -> &str`
*   **Description**: LLM の出力テキストに含まれるマークダウンコードブロックのノイズ文字（例: ` ```json `）をトリミング除去します。

#### `parse_proposal_json`
*   **Signature**: `fn parse_proposal_json(raw: &str) -> Result<AffectProposal, ClassifierError>`
*   **Description**: クリーニングされた応答 JSON テキストをデシリアライズし、数値データにマッピングします。

#### `clamp_absolute`
*   **Signature**: `const fn clamp_absolute(v: f32, min: f32, max: f32) -> f32`
*   **Description**: 数値が感情アプレザルの許容指標限界に適合するか検証し、クランプ処理を施します。

---

## 6. タイプ結合用ヘルパー (`types.rs`)

#### `with_proposal`
*   **シグネチャ**: `pub fn with_proposal(mut self, proposal: AffectProposal) -> Self`
*   **説明**: 処理用の `TurnAffectInput` 構造体に、保留されていた分類器の感情提案（`AffectProposal`）データをバインドして返します。
