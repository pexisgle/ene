# `MessageBuildContext` & プロンプトフォーマット仕様

`message_builder` モジュールは、LLMに送信する対話コンテキストおよびシステムプロンプトのフォーマット組み立てを担当します。特に、デスクトップ上に常駐するAIマスコットとしての制約条件や、アバターアニメーション制御用の表情マーカー `<|perf:expr=NAME|>` の出力プロトコルを定義する役割を持ちます。

---

## 1. 入力パラメータ構造体

### `MessageBuildContext<'a>` (公開 / 構造体)
メッセージリスト構築に必要なすべてのコンテキストを保持します。
*   `card: &'a CharacterCardV3`: キャラクター定義情報。
*   `user_input: &'a str`: 今回のユーザー入力テキスト。
*   `history: &'a [ene_mind::HistoryEntry]`: 対話履歴エントリ。
*   `runtime_context: Option<&'a str>`: ツール結果など、ユーザー入力の直前に挿入される追加コンテキスト。
*   `runtime_rules: &'a str`: システムプロンプトの上部に挿入される振る舞いルール。
*   `user_name: &'a str`: ユーザーの表示名。
*   `prompts: &'a PromptLibrary`: 使用言語に対応したプロンプト辞書。
*   `emotion_enabled: bool`: 感情エンジンが有効かどうか（表情プロトコルの選択に影響）。

---

## 2. システムプロンプトアセンブリ

### `build_system_prompt`
*   **シグネチャ**:
    ```rust
    pub fn build_system_prompt(
        card: &CharacterCardV3,
        runtime_rules: &str,
        user_name: &str,
        prompts: &PromptLibrary,
    ) -> String
    ```
*   **レイアウト構成**:
    以下の順序でテキストブロックを結合します。
    1.  **マスコットコンテキストフレーム**:
        `prompts.system().render_mascot_context(char_name, user_name)` を呼び出し、「あなたはデスクトップAIコンパニオンである」という overlay 表示制約（短く応答する、PC上の状態を意識するなど）を最上部に配置します。LLMが文脈を解釈する際、最も重要な前提知識として扱われます。
    2.  **実行ルール (Runtime Rules)**:
        `runtime_rules`（空の場合はデフォルトの `DEFAULT_RUNTIME_RULES`）を結合します。
    3.  **キャラクターアイデンティティ**:
        カード定義の `system_prompt`、`personality`、`description` を連結します。
    4.  **現在のシーン**:
        カード定義の `scenario` を連結します。
*   **CBSマクロ展開**:
    組み立てた最終プロンプトに対して `expand_cbs_macros` を適用し、文中の `{{char}}` をキャラクター名に、`{{user}}` をユーザー表示名に置換します。

---

## 3. 出力制御コントラクト (PHI)

Ene では、LLMの応答に表情タグを含める「インライン制御」と、エンジン側で感情値を査定する「自動制御」の2つのモードがあります。

### `build_expression_phi`
*   **シグネチャ**: `pub fn build_expression_phi(card: &CharacterCardV3, prompts: &PromptLibrary) -> Option<String>`
*   **解説**: 表情インライン制御モード用（感情エンジン無効時）。LLMに対して、感情に応じた表情マーカータグ（例: `<|perf:expr=joy|>`）を応答テキストの先頭に含めるよう指示するプロンプトを構築します。
*   **機能**: `resolve_expressions(card)` を呼び出してキャラクターが対応している表情名（例: `joy`, `sad`, `angry`, `surprised`）を抽出し、それらだけを出力可能な有効値としてLLMに明示します。

### `build_natural_dialogue_contract`
*   **シグネチャ**: `pub fn build_natural_dialogue_contract(card: &CharacterCardV3, prompts: &PromptLibrary, user_name: &str) -> Option<String>`
*   **解説**: 表情自動制御モード用（感情エンジン有効時）。LLMに対し、表情タグなどの特殊な制御コードを応答に**含めない**ように指示します（表情はターン終了後にエンジン側で自動調停するため）。

### `build_cognitive_output_contract`
*   **シグネチャ**:
    ```rust
    pub fn build_cognitive_output_contract(
        card: &CharacterCardV3,
        prompts: &PromptLibrary,
        emotion_enabled: bool,
        user_name: &str,
    ) -> Option<String>
    ```
*   **解説**: `emotion_enabled` が `true` の場合は `build_natural_dialogue_contract` を、`false` の場合は `build_expression_phi` を選択して返します。

---

## 4. メッセージリスト生成

### `build_messages`
*   **シグネチャ**: `pub fn build_messages(ctx: &MessageBuildContext<'_>) -> Result<Vec<LlmMessage>, EneRuntimeError>`
*   **連結フロー**:
    1.  `System`: `build_system_prompt` で生成したシステムプロンプト。
    2.  `System`: 初回ターンのみ、キャラクターカード定義の `mes_example`（例文メッセージ）を追加。
    3.  履歴 (`HistoryEntry`): 会話履歴リストを `User` と `Assistant` のロールで交互に追加。
    4.  `System`: `build_cognitive_output_contract` で取得した出力制御コントラクトを追加（直前の対話履歴の後ろに配置することで、出力フォーマットの厳格な遵守を促します）。
    5.  `User`: 現在のユーザー入力。`runtime_context` があれば、ユーザー入力の直前に `[Runtime Context]\n{context}` ブロックとして差し込みます。
