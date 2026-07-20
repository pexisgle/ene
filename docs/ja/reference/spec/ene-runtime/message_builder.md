# コンテキストプロンプト構築仕様 (`message_builder`)

`message_builder` は、アクターセッションのコンテキスト情報（キャラクター特性、PAD 感情、履歴ログ、RAG を通じて取得されたロングタームメモリなど）を統合して、LLM に入力する最終的なプロンプトパケットを生成します。

---

## 1. コアメッセージ作成メソッド

#### `build_messages`
*   **シグネチャ**: `pub async fn build_messages(session: &ConversationSession, mind: &CognitionEngine, pre_turn: &PreTurnOutput, budget: &ContextBudget, config: &MindConfig) -> Result<Vec<LlmMessage>, CognitionError>`
*   **プロセス**:
    1.  `build_system_prompt` を呼び出し、キャラクターの核となるシステムプロンプト指示テキストを構築します。
    2.  `LlmMessage::System` を作成し、構築したシステムプロンプトテキストをメッセージ配列の最初の要素として挿入します。
    3.  セッション内の直近の会話履歴ログ（`session.history`）を読み取ります。
    4.  各履歴レコードを `LlmMessage` にマッピングし、配列に順次追加します。
    5.  構築したメッセージ配列を返します。

#### `build_system_prompt`
*   **シグネチャ**: `pub async fn build_system_prompt(session: &ConversationSession, mind: &CognitionEngine, pre_turn: &PreTurnOutput, budget: &ContextBudget) -> Result<String, CognitionError>`
*   **プロセス**:
    1.  `CognitionEngine::compose_prompt_packet` を呼び出し、システムプロンプトを構築します。
    2.  ユーザー名およびキャラクター名の変数情報をキャラクターカードテンプレートに挿入し、CBS マクロ（`expand_cbs_macros`）を展開します。
    3.  `pre_turn` 内の PAD 感情スコアと現在の気分（mood）ラベルをプロンプトにバインドします。
    4.  RAG 経由で取得したロングタームメモリの項目群を優先度に基づいてパッケージングします。
    5.  結果を構造化テキストとしてシリアライズして返します。

---

## 2. メッセージパッケージングと優先度の規則

最終的なプロンプトは、キャラクターの制限トークンに収まるように優先度（`ContextBudget`）に基づいてパックされます。

*   **ユーザー入力 / プラットフォーム制約 (最優先)**: トークン制限の影響を受けず、プロンプトに必ず含まれます。
*   **キャラクターカーネルアイデンティティ**: キャラクターの核となる性格プロンプト。トークン制限下でも保護されます。
*   **出力・表情制御の契約**: mascot アニメーションの表情を制御するタグの規則指示テキスト。
*   **振る舞いに関する契約**: 原作者などのクリエイター指示テキスト。
*   **直近の会話履歴**: 最低限必要な履歴ウィンドウ分は必ず保護されます。トークン制限を超えた分は古いものから順に切り詰め（トリミング）されます。
*   **感情・シーンのサマリー**: 現在の気分情報と、履歴圧縮タスクで要約されたシーン情報。
*   **アクティブなコミットメント**: ユーザーと約束したタスクのリスト。
*   **想起したメモリ (低優先)**: トークン制限を超えた場合、適合度（類似度）スコアの低いメモリ項目から順に破棄されます。
*   **スタイル表現のサンプル (最優先で破棄)**: トークン制限を超えた場合、最初に完全に削除されます。

---

## 3. ヘルパー関数仕様

#### `cbs_macro_transform`
*   **シグネチャ**: `fn cbs_macro_transform(raw_text: &str, character: &str, user: &str) -> String`
*   **説明**: キャラクターカード記述内の `{{char}}` や `{{user}}` のプレースホルダーパラメータを、キャラクターの表示名とユーザーの構成名に置換します。

#### `pack_recalled_memories`
*   **シグネチャ**: `fn pack_recalled_memories(memories: &[RecalledMemory], max_tokens: usize) -> (String, usize)`
*   **説明**: 想起されたメモリリストを指定されたトークンサイズ制限内に収まるようにパッキングし、フォーマットされたテキストと合計トークン数を返します。
