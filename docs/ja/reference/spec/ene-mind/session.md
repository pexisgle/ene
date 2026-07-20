# `ConversationSession` & セッション状態仕様

本ドキュメントでは、対話履歴の蓄積、表示バッファの管理、キャラクターカード情報（自己認識およびアセット定義）、およびストリームからの表情・モーションマーカー `<|perf:…|>` の抽出処理を担うセッション管理の仕様を定義します。

---

## 1. データ構造

### `ConversationSession` (公開 / 構造体)
対話セッションの情報を一元的に保持するメモリ上のステートホルダー。
*   **メンバ変数**:
    -   `character_card: Option<CharacterCardV3>`: ロードされたキャラクター情報。
    -   `history: Vec<HistoryEntry>`: 会話履歴（User / Assistant / System の交互シーケンス）。
    -   `display_buffer: String`: UI 表示用にマーカーや不要コードを除去した、整形済み応答バッファ。
    -   `memory: MemoryContext`: メモリストアおよび埋め込みプロバイダーの参照。
    -   `session_id: SessionId`: セッションごとに生成される UUID 文字列ベースのID。
*   **主要メソッド**:
    -   `set_card(&mut self, card: &CharacterCardV3)`: セッションにキャラクターカードをバインドし、固有のメモリハッシュなどを初期化。
    -   `add_user_message(&mut self, text: String)`: 会話履歴に `User` のメッセージを追加。
    -   `add_assistant_message(&mut self, text: String)`: 会話履歴に `Assistant` のメッセージを追加。

---

## 2. 特殊トークン・表情マーカーパース (`special_token.rs`)

Ene では、LLM が対話テキストの中にインライン形式でアバターのアニメーション制御タグ `<|perf:…|>` を差し込むことができます（感情エンジンが無効な場合）。

### 1. マーカープロトコル構文
*   表情指示: `<|perf:expr=EXPRESSION_NAME|>`
*   モーション指示: `<|perf:motion=MOTION_NAME|>`

### 2. 主要関数

#### `split_text_and_special_tokens`
*   **シグネチャ**: `pub fn split_text_and_special_tokens(text: &str) -> (String, Vec<String>)`
*   **解説**: LLM からの入力チャンクから、プレーンな発話テキスト部分と、`<|perf:…|>` で囲まれた特殊トークン部分を分離して返却します。プレーンテキストは `display_buffer` や `EneEvent::TextDelta` の配信用に使用されます。

#### `parse_performance_marker`
*   **シグネチャ**: `pub fn parse_performance_marker(marker: &str) -> Option<PerformanceCue>`
*   **解説**: 特殊トークン文字列（例: `<|perf:expr=joy|>`）をパースし、型定義された `PerformanceCue`（種類: `PerfKind::Expression`, 値: `joy` など）へと変換します。パース失敗時は `None` を返します。

#### `strip_markers`
*   **シグネチャ**: `pub fn strip_markers(text: &str) -> String`
*   **解説**: テキスト内のすべてのインラインパフォーマンスマーカーを正規表現で削除した綺麗な会話文字列を返します。データベースに履歴を保存する直前のクリーニングに用いられます。

---

## 3. キャラクターカードV3 (CharacterCardV3) の連携

Ene はキャラクターカード（ Tavern などの共通仕様 CCv3 形式）を直接読み込んで振る舞いを制御します。

*   **CBS マクロ展開 (`expand_cbs_macros`)**:
    プロンプト内やカードの背景文に含まれる `{{char}}` をキャラクターカードの `name` フィールド値に置換し、`{{user}}` を設定ファイルに記述された `user_name` に動的に置換します。
*   **表情定義の解決 (`resolve_expressions`)**:
    キャラクターカード内の `expressions` セクションに定義された VRM BlendShape（喜怒哀楽などの重み値）のバインド定義を読み込み、ランタイムに利用可能な表情カタログ（`ResolvedExpression` リスト）としてパース・解決します。
