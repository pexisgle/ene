# AI-Core 動作フロー ドキュメント

## 概要

Ene の AI-Core は、LLM との対話、ツール呼び出し、長期記憶、セッション管理を統合した Rust 製のコアライブラリです。

---

## 1. プロジェクト構成

```
crates/
├── ene-ai-core/    # AI コアライブラリ（LLM連携、ツール、記憶、サンドボックス）
├── ene-app/        # Bevy ベースの GUI デスクトップアプリ（VRM キャラクターオーバーレイ）
└── ene-cli/        # テスト・直接AI対話用インタラクティブCLI
```

---

## 2. コンポーネント関係図

```mermaid
graph TB
    subgraph Config
        AiSettings["AiSettings<br/>（マスター設定）"]
        MemorySettings["AiMemorySettings<br/>（長期記憶設定）"]
        SandboxSettings["AiSandboxSettings<br/>（セキュリティ設定）"]
        McpConfig["McpServerConfig<br/>（MCPサーバー設定）"]
    end

    subgraph Session
        ConversationSession["ConversationSession<br/>（会話状態管理）"]
        CharacterCard["CharacterCardV3<br/>（キャラクターカード）"]
    end

    subgraph Core
        StreamEngine["run_ai_with_tools()<br/>（ストリーミングエンジン）"]
        PromptBuilder["prompt_builder<br/>（メッセージ構築）"]
        Client["client.rs<br/>（OpenAIクライアント構築）"]
        Error["error.rs<br/>（AiCoreError）"]
    end

    subgraph Memory
        MemoryStore["MemoryStore<br/>（SQLite + sqlite-vec + Diesel）"]
        Embedding["EmbeddingProvider Trait"]
        ApiEmbedding["ApiEmbeddingProvider<br/>（OpenAI互換API）"]
        GgufEmbedding["GgufEmbeddingProvider<br/>（ローカルGGUFモデル/Candle）"]
        Summarizer["summarizer<br/>（LLM要約）"]
        SessionFuncs["conversation_manager.rs<br/>（セッション境界関数群）"]
    end

    subgraph Tools
        ToolRegistry["ToolRegistry Trait"]
        ToolRegistryBuilder["ToolRegistryBuilder<br/>（tool_factory）"]
        BuiltinTools["BuiltinToolRegistry<br/>（時間・システム情報）"]
        OpencodeTools["OpencodeToolRegistry<br/>（filesystem/shell/browser/app/web等）"]
        ScreenshotTools["ScreenshotToolRegistry<br/>（スクリーンキャプチャ）"]
        McpTools["McpToolRegistry<br/>（外部MCPサーバー）"]
        CompositeTools["CompositeToolRegistry<br/>（統合レジストリ）"]
        UndoManager["UndoManager<br/>（ファイル操作取り消し）"]
    end

    subgraph Sandbox
        SandboxConfig["SandboxConfig<br/>（パス・コマンド制限）"]
        PermissionGate["PermissionGate<br/>（権限チェック）"]
    end

    AiSettings --> ConversationSession
    MemorySettings --> MemoryStore
    SandboxSettings --> SandboxConfig
    McpConfig --> McpTools

    ConversationSession <--> PromptBuilder
    PromptBuilder <--> CharacterCard
    ConversationSession <--> StreamEngine
    StreamEngine --> PromptBuilder
    StreamEngine --> ToolRegistry
    StreamEngine --> Client
    Client --> StreamEngine

    ConversationSession --> MemoryStore
    ConversationSession --> Embedding
    SessionFuncs --> MemoryStore
    SessionFuncs --> Summarizer
    ApiEmbedding --> Embedding
    GgufEmbedding --> Embedding
    Embedding --> MemoryStore

    ToolRegistry --> BuiltinTools
    ToolRegistry --> OpencodeTools
    ToolRegistry --> ScreenshotTools
    ToolRegistry --> McpTools
    ToolRegistry --> CompositeTools
    ToolRegistryBuilder --> OpencodeTools
    OpencodeTools --> UndoManager

    CompositeTools --> BuiltinTools
    CompositeTools --> OpencodeTools
    CompositeTools --> ScreenshotTools
    CompositeTools --> McpTools

    OpencodeTools --> SandboxConfig
    SandboxConfig --> PermissionGate
```

---

## 3. リクエスト処理フロー

### 3.1 全体フロー

```mermaid
sequenceDiagram
    participant User as ユーザー
    participant SessionFuncs as conversation_manager.rs<br/>（境界関数）
    participant Session as ConversationSession
    participant Stream as run_ai_with_tools()
    participant Prompt as prompt_builder
    participant LLM as LLM API
    participant Tools as ToolRegistry
    participant Memory as MemoryStore
    participant Consumer as コンシューマー<br/>（ai_bridge / CLI main）

    User->>SessionFuncs: 入力メッセージ
    SessionFuncs->>SessionFuncs: セッション境界チェック<br/>（タイムアウト/トピック変更）
    alt セッション分割が必要
        SessionFuncs->>Memory: 会話ログ保存
        SessionFuncs->>LLM: 要約生成
        LLM-->>SessionFuncs: 要約結果
        SessionFuncs->>Memory: 要約・重要事実を保存
        SessionFuncs->>Session: セッションリセット
    end

    Session->>Session: ユーザー入力埋め込みタスク生成（tokio::spawn）
    Session->>Session: ユーザーメッセージを履歴に追加
    Session->>Stream: run_ai_with_tools() 呼び出し

    Stream->>Stream: ユーザー入力ログを非保存（tokio::spawn）
    Stream->>Stream: Tool RAG判定<br/>（tool_rag_enabled?）
    alt Tool RAG有効
        Stream->>Tools: ensure_index_built<br/>（ツール埋め込み構築）
        Stream->>Memory: ユーザー入力を埋め込み
        Stream->>Tools: list_relevant_tools<br/>（コサイン類似度で絞り込み）
        Stream->>Stream: always_includeツールを追加
    else Tool RAG無効
        Stream->>Tools: list_tools（全ツール）
    end
    Stream->>Memory: 重要事実を取得
    Stream->>Memory: 関連要約をベクトル検索
    Stream->>Prompt: メッセージ構築
    Prompt-->>Stream: 完全なメッセージ配列

    loop ツール呼び出しラウンド（最大10回）
        Stream->>LLM: チャット補完リクエスト（ストリーミング）
        LLM-->>Stream: テキスト/ツール呼び出しストリーム
        alt テキスト応答
            Stream-->>Consumer: TextDelta イベント（rawテキスト）
        else ツール呼び出し
            Stream->>Tools: ツール実行
            Tools-->>Stream: 実行結果
            Stream->>Stream: ツール結果をメッセージに追加
        end
    end

    Stream->>Stream: アシスタントログをメモリに保存（tokio::spawn）
    Stream-->>Session: 応答完了
    Session->>Session: 応答を履歴に追加
    Session-->>Consumer: 最終応答

    Note over Consumer: SpecialToken解析は<br/>コンシューマー側で実施<br/>（ai_bridge / CLI main）
```

### 3.2 AiStreamEvent 一覧

| イベント | 説明 |
|----------|------|
| `TextDelta(String)` | テキスト断片（raw） |
| `SpecialToken(String)` | 感情表現トークン（emo） |
| `ToolCallStart { name, arguments }` | ツール呼び出し開始 |
| `ToolCallResult { name, result }` | ツール実行結果 |
| `PermissionRequired { request_id, action, target, description }` | パーミッション要求（Phase 2） |
| `TaskProgress { task_id, step, total_steps, description }` | タスク進捗（Phase 2） |
| `SessionSplit { summary, reason }` | セッション分割通知 |
| `Finished` | 応答完了 |
| `Error(String)` | エラー |

### 3.2 ツール呼び出しループ詳細

```mermaid
flowchart TD
    A[ツール呼び出しループ開始] --> B{Tool RAG有効?}
    B -->|はい| C[ツール埋め込みインデックス構築]
    C --> D[ユーザー入力を埋め込み]
    D --> E[list_relevant_tools<br/>コサイン類似度で絞り込み]
    E --> F[always_includeツールを追加]
    B -->|いいえ| G[list_tools 全ツール取得]
    F --> H[OpenAI互換クライアント作成]
    G --> H
    H --> I[メモリコンテキスト取得<br/>重要事実 + 関連要約]
    I --> J[prompt_builderでメッセージ構築]
    J --> K[ツール定義をOpenAI形式に変換]
    K --> L[チャット補完リクエスト送信]
    L --> M{ストリーミング応答}

    M -->|TextDelta| N[テキスト処理（rawテキスト配信）]
    N --> O[TextDelta イベント出力]

    M -->|ToolCalls| P[ツール呼び出し蓄積]
    P --> Q[ストリーム終了]

    O --> Q
    Q --> R{tool_calls 存在?}

    R -->|はい| S[アシスタントメッセージ作成]
    S --> T[各ツール呼び出し実行]
    T --> U[ToolCallStart イベント出力]
    U --> V[registry.call_tool 実行]
    V --> W[ToolCallResult イベント出力]
    W --> X{スクリーンショット?}
    X -->|はい| Y[画像をメッセージに注入]
    X -->|いいえ| Z[ツールメッセージ作成]
    Y --> Z
    Z --> AA[メッセージ配列に追加]
    AA --> AB{ラウンド数 < 上限?}
    AB -->|はい| L
    AB -->|いいえ| AC[ループ終了]

    R -->|いいえ| AD[アシスタントログをメモリに保存（tokio::spawn）]
    AD --> AE[SessionSplit イベント出力（該当時）]
    AE --> AF[Finished イベント出力]
    AF --> AG[応答完了]
    AC --> AG
```

---

## 4. セッション自動分割フロー

```mermaid
flowchart TD
    A[ユーザー入力受信] --> B[spawn_split_task<br/>バックグラウンドタスク生成]
    B --> C[check_boundary 実行]

    C --> D{タイムアウトチェック}
    D -->|経過時間 >= 設定値<br/>かつ ターン数 >= 最小値| E[Split: Timeout]
    D -->|該当なし| F{トピック変更チェック}

    F -->|cosine_similarity < 閾値| G[Split: TopicChange]
    F -->|該当なし| H[Continue]

    E --> I[execute_split 実行]
    G --> I

    I --> J[会話ログをMemoryStoreに保存]
    J --> K[既存の重要事実を取得]
    K --> L[LLMで会話要約生成<br/>（JSONスキーマ）]
    L --> M[セッションメッセージをMax-pooling埋め込み]
    M --> N[要約 + 重要事実をMemoryStoreに挿入]
    N --> O[新しいsession_id生成]
    O --> P[oneshotチャンネルで結果送信]

    H --> Q[通常続行]

    P --> R[poll_split_result で結果確認]
    R --> S[セッションリセット<br/>新session_id設定]
```

---

## 5. 長期記憶システム

### 5.1 メモリ初期化フロー

```mermaid
flowchart TD
    A[init_memory 呼び出し] --> B[DBパス解決<br/>（キャラクターカードパスから派生）]
    B --> C[親ディレクトリ作成]
    C --> D[MemoryStore::open]
    D --> E[sqlite-vec 拡張初期化]
    E --> F[Dieselマイグレーション実行<br/>conversation_summaries<br/>conversation_keyfacts<br/>conversation_logs<br/>tool_embeddings]
    F --> G{embedding_provider_type}
    G -->|Api| H[ApiEmbeddingProvider 作成<br/>（OpenAI互換API）]
    G -->|Local| I[GgufEmbeddingProvider 作成<br/>（GGUFモデル/Candle）]
    H --> J[session.init_memory 実行]
    I --> J
```

### 5.2 メモリ検索フロー

```mermaid
flowchart TD
    A[ユーザー入力] --> B[埋め込みベクトル生成<br/>（pending_embedding）]
    B --> C[MemoryStore.search_summaries]
    C --> D[sqlite-vec でコサイン類似度検索]
    D --> E{similarity >= 閾値?}
    E -->|はい| F[関連要約を返却]
    E -->|いいえ| G[該当なし]
    F --> H[プロンプトに注入]
    G --> H
    H --> I[LLMリクエスト送信]
```

> **Tool RAG検索フロー**: ユーザー入力を埋め込み → `tool_embeddings`テーブルからコサイン類似度でツールを検索 → `tool_rag_limit`件に絞り込み → `tool_rag_always_include`のツールを追加

### 5.3 メモリテーブル構造

```mermaid
erDiagram
    conversation_summaries {
        INTEGER id PK "AUTOINCREMENT"
        TEXT session_id
        TEXT card_name
        TEXT summary
        TEXT embedding "BLOB"
        TEXT ended_at
        DATETIME created_at
    }

    conversation_keyfacts {
        INTEGER id PK
        TEXT card_name
        INTEGER summary_id
        TEXT key
        TEXT value
        DATETIME created_at
    }

    conversation_logs {
        INTEGER id PK
        TEXT session_id
        TEXT card_name
        TEXT role
        TEXT content
        DATETIME created_at
    }

    tool_embeddings {
        TEXT tool_name PK
        TEXT version_hash
        TEXT embedding "BLOB"
        DATETIME created_at
    }

    conversation_summaries ||--o{ conversation_keyfacts : "summary_id"
```

---

## 6. プロンプト構築フロー

```mermaid
flowchart LR
    A[build_messages 呼び出し] --> B[1. システムメッセージ<br/>（ランタイムルール + キャラクターカード）]
    B --> C[2. 例メッセージ<br/>（キャラクターカードから、初回ターンのみ）]
    C --> D[3. 想起された要約<br/>（メモリ有効時）]
    D --> E[4. 重要事実<br/>（メモリ有効時）]
    E --> F[5. 会話履歴<br/>（ユーザー/アシスタント交互）]
    F --> G[6. 表現プロトコル<br/>（CBS式展開 + emoトークン指示）]
    G --> H[7. 現在のユーザーメッセージ<br/>（ランタイムコンテキスト付き）]
```

---

## 7. ツールシステム

### 7.1 ツールレジストリ構成

```mermaid
classDiagram
    class ToolRegistry {
        <<trait>>
        +list_tools() Vec~ToolDefinition~
        +list_relevant_tools(query_embedding, limit) Vec~ToolDefinition~
        +call_tool(name, args) Future~Result&lt;String, String&gt;~
        +set_session_id(session_id) "デフォルト実装: no-op"
        +ensure_index_built(embedder, store) Future~Result&lt;void, String&gt;~
    }

    class ToolDefinition {
        +name: String
        +description: String
        +parameters: Value
        +category: ToolCategory
        +keywords: Vec~String~
        +embedding_text() String
    }

    class ToolCategory {
        <<enumeration>>
        Filesystem
        Shell
        Browser
        App
        WebSearch
        Utility
    }

    class ToolRegistryBuilder {
        +with_builtin()
        +with_screenshot(scale_percent)
        +with_sandbox(config)
        +with_sandbox_settings(settings)
        +with_store(store)
        +add_registry(registry)
        +build() Arc~ToolRegistry~
    }

    class BuiltinToolRegistry {
        +get_current_time
        +get_system_info
    }

    class OpencodeToolRegistry {
        -sandbox: SandboxConfig
        -undo_manager: UndoManager
        -session_id: String
        -todo_store: TodoStore
        -browser_store: BrowserSessionStore
        +filesystem (read/write/edit/delete/glob/grep/patch)
        +shell
        +browser (navigate/click/type/wait/screenshot/get_content/scroll/close)
        +app (list_windows/focus_window/type_text/press_key/mouse_move/mouse_click/clipboard)
        +webfetch
        +websearch (duckduckgo/tavily/brave)
        +undo
        +todo
        +question
    }

    class ScreenshotToolRegistry {
        +take_screenshot
    }

    class McpToolRegistry {
        +dynamic_tools
    }

    class CompositeToolRegistry {
        -registries: Vec~Box~
        -store: Option~MemoryStore~
        +list_tools()
        +list_relevant_tools()
        +call_tool()
        +ensure_index_built()
    }

    ToolRegistry <|.. BuiltinToolRegistry
    ToolRegistry <|.. OpencodeToolRegistry
    ToolRegistry <|.. ScreenshotToolRegistry
    ToolRegistry <|.. McpToolRegistry
    ToolRegistry <|.. CompositeToolRegistry
    CompositeToolRegistry *-- ToolRegistry
    ToolRegistryBuilder --> OpencodeToolRegistry
    ToolDefinition --> ToolCategory
```

### 7.2 OpencodeToolRegistry ツール一覧

| ツール | 説明 |
|--------|------|
| `filesystem` | 統一ファイルシステム操作（action: read/write/edit/delete/glob/grep/patch）。readは行番号付き出力、offset/limit対応。writeは事前読み込み必須、Undoエントリ作成。editは9段階の置換戦略（simple/line_trimmed/block_anchor/whitespace_normalized/indentation_flexible/escape_normalized/trimmed_boundary/context_aware/multi_occurrence）、Levenshtein距離計算、ファイルロック付き。patchはマルチファイルパッチ適用 |
| `shell` | コマンド実行（タイムアウト、作業ディレクトリ指定可能、サンドボックス適用） |
| `browser` | Chromiumブラウザ自動化（CDP）。action: navigate/click/type/wait/screenshot/get_content/scroll/close。セッションごとに状態永続化。get_contentはmarkdown/html形式、trim/extractオプションでトークン削減 |
| `app` | OSレベルGUI自動化（enigo/xcap）。action: list_windows/focus_window/type_text/press_key/mouse_move/mouse_click/clipboard_read/clipboard_write |
| `webfetch` | URLコンテンツ取得（format/timeout指定可能） |
| `websearch` | ウェブ検索（backend: duckduckgo/tavily/brave、limit指定可能） |
| `undo` | 直前のファイル操作を取り消し（セッションごとUndoスタック） |
| `todo` | タスクリスト管理（セッションごとTodoStore） |
| `question` | ユーザーへの質問 |

---

## 8. セキュリティサンドボックス

```mermaid
flowchart TD
    A[コマンド実行リクエスト] --> B{サンドボックス有効?}
    B -->|いいえ| C[直接実行]
    B -->|はい| D[パス正規化]
    D --> E{許可ディレクトリ内?}
    E -->|いいえ| F[エラー: パス拒否]
    E -->|はい| G{書き込み操作?}
    G -->|はい| H{書き込み可能ディレクトリ内?}
    H -->|いいえ| I[エラー: 書き込み拒否]
    H -->|はい| J[コマンドブロックチェック]
    G -->|いいえ| J
    J --> K{ブロックパターンに一致?}
    K -->|はい| L[エラー: コマンド拒否]
    K -->|いいえ| M[リソース制限チェック]
    M --> N{制限内?}
    N -->|いいえ| O[エラー: リソース超過]
    N -->|はい| P[実行許可]
```

### サンドボックス制限

| 項目 | 制限値 |
|------|--------|
| 最大読み込みバイト数 | 50KB |
| 最大書き込みバイト数 | 1MB |
| シェルタイムアウト | 120秒 |
| 最大シェル出力バイト数 | 設定値 |
| 最大シェル出力行数 | 設定値 |

### ブロックコマンドパターン（正規表現）

- `r"rm\s+-rf\s+/"`
- `r"dd\s+if="`
- `r"mkfs"`
- フォークボム（`:(){ :|:& };:` → `r":\s*\{\s*\|\s*&\s*;\s*\}"`）

> **注意**: `AiSandboxSettings::default()` には `sudo` は含まれない。明示的に設定に追加しない限り、`sudo` はブロックされない。

---

## 9. 感情表現トークン（Special Token）

```mermaid
flowchart TD
    A[ストリーミングテキスト受信<br/>（run_ai_with_tools から TextDelta）] --> B[コンシューマー側で解析<br/>（ai_bridge.rs / CLI main.rs）]
    B --> C[split_text_and_special_tokens]
    C --> D{<|...|> トークン存在?}
    D -->|はい| E[extract_emotion_from_token]
    D -->|いいえ| F[通常テキストとして処理]
    E --> G[emo:以降の感情名を抽出]
    G --> H[SpecialToken イベント出力]
    H --> I[VRM表情更新]

    style A fill:#f9f,stroke:#333
    style B fill:#ff9,stroke:#333
```

> **注意**: `stream.rs` 自体は SpecialToken の解析を行わない。`TextDelta(String)` を raw テキストで配信し、解析はコンシューマー側（`ai_bridge.rs` / CLI `main.rs`）で実施される。

### Emotion Token Format

```
<|emo:happy|>
```

> CBS式の`<|emo:<name>|>`は、キャラクターカードの`post_history_instructions`または自動生成されたEmotion Expression ProtocolによりLLMに指示される。利用可能な感情一覧はカードの`expressions`フィールドから展開される。

---

## 10. アプリケーション起動フロー

### 10.1 GUI（ene-app）

```mermaid
flowchart TD
    A[main 関数] --> B[read_cli_paths<br/>VRM/VRMAパス取得]
    B --> C[CharacterSettings::discover<br/>アセットスキャン]
    C --> D[App::new プラグイン初期化]
    D --> E[AiPlugin::build]
    E --> F[AiRequestEvent / AiStreamEvent 登録]
    F --> G[AiRuntimeState 初期化<br/>（ConversationSession + pending_split）]
    G --> H[AiTokioRuntime 初期化<br/>（マルチスレッドTokio）]
    H --> I[システム登録<br/>enqueue_ai_requests -> process_embedding -> start_next_ai_request -> poll_ai_worker]
    I --> J[初回AIリクエスト待機]
    J -.-> K[メモリ初期化は初回AIリクエスト時に遅延実行<br/>（start_next_ai_request 内）]
```

### 10.2 CLI（ene-cli）

```mermaid
flowchart TD
    A[main 関数] --> B[settings.json読み込み]
    B --> C[ConversationSession作成<br/>キャラクターカード読み込み]
    C --> D{memory.enabled?}
    D -->|はい| E[init_memory<br/>MemoryStore + EmbeddingProvider]
    D -->|いいえ| F[メモリ無効]
    E --> G{--tooltest?}
    F --> G
    G -->|はい| H[ツールテスト実行して終了]
    G -->|いいえ| I[ツールレジストリ構築]
    I --> J[ToolRegistryBuilder]
    J --> K[with_builtin]
    J --> L[with_screenshot]
    J --> M[with_sandbox_settings<br/>（サンドボックス有効時）]
    J --> N[with_store<br/>（メモリ有効時）]
    K --> O[CompositeToolRegistry統合]
    L --> O
    M --> O
    N --> O
    O --> P[インタラクティブループ開始]
    P --> Q[ユーザー入力読み込み<br/>（dialoguer::Input）]
    Q --> R[コマンド処理<br/>（/help, /config, /card等）]
    R --> S[セッション境界チェック]
    S --> T[埋め込みタスク生成]
    T --> U[run_ai_with_tools 実行]
    U --> V[イベントストリーム出力]
```

---

## 11. デザインパターン

| パターン | 説明 |
|----------|------|
| **Traitベースツールシステム** | `ToolRegistry` トレイトにより、プラグ可能なツールソース（builtin、MCP、ファイルシステム、スクリーンショット、ブラウザ、GUI自動化） |
| **コンポジットパターン** | `CompositeToolRegistry` が複数のレジストリを1つに統合 |
| **Tool RAG** | `ToolDefinition`の`keywords`と`category`から埋め込みを生成、`tool_embeddings`テーブルに保存。ユーザー入力とのコサイン類似度で動的にツールを選択しtoken消費を抑制 |
| **非同期ストリーミング** | `tokio_stream::Stream` によるAI応答の逐次配信 |
| **イベント駆動アーキテクチャ（GUI）** | Bevyメッセージ（`AiRequestEvent`、`AiStreamEvent`）でAI処理とレンダリングを分離 |
| **バックグラウンドタスクパターン** | セッション分割評価をバックグラウンドTokioタスクで実行、oneshotチャンネルで結果ポーリング |
| **ベクトルメモリ** | `sqlite-vec` 拡張 + Diesel ORMによるコサイン類似度検索 |
| **GGUFローカル埋め込み** | CandleフレームワークによるGGUF量子化モデルのローカル推論（`GgufEmbeddingProvider`） |
| **メッセージMax-pooling** | セッションメッセージを個別にembedし、各次元で最大値を採用。挨拶などの情報量ゼロのメッセージによる意味の希釈を防止 |
| **Undoシステム** | セッションごとのUndoスタックでファイル操作を追跡・ロールバック |
| **サンドボックスセキュリティ** | パス正規化 + プレフィックスマッチング + 正規表現コマンドブロック |
| **CBS式展開** | キャラクターカードの`{{char}}`/`{{user}}`式を自動展開 |

---

## 12. 主要ファイル一覧

| ファイル | 目的 |
|----------|------|
| `crates/ene-ai-core/src/lib.rs` | クレートルート（再エクスポート） |
| `crates/ene-ai-core/src/config.rs` | 設定構造体（AiSettings、Memory、Sandbox、MCP） |
| `crates/ene-ai-core/src/session.rs` | ConversationSession（会話状態管理） |
| `crates/ene-ai-core/src/stream.rs` | run_ai_with_tools（ストリーミングエンジン + Tool RAG） |
| `crates/ene-ai-core/src/prompt_builder.rs` | メッセージ構築 + CBS式展開 + 表現プロトコル |
| `crates/ene-ai-core/src/character_card.rs` | キャラクターカード解析（V3形式、CBSマクロ展開） |
| `crates/ene-ai-core/src/client.rs` | OpenAIクライアント構築（build_openai_client） |
| `crates/ene-ai-core/src/error.rs` | AiCoreError エラー型 |
| `crates/ene-ai-core/src/utils.rs` | ユーティリティ（truncate、init_memory） |
| `crates/ene-ai-core/src/paths.rs` | パス解決ユーティリティ |
| `crates/ene-ai-core/src/resources.rs` | リソース管理 |
| `crates/ene-ai-core/src/schema.rs` | Dieselスキーマ定義（自動生成） |
| `crates/ene-ai-core/src/embedding/mod.rs` | テキスト埋め込みプロバイダー（Api/Gguf） |
| `crates/ene-ai-core/src/embedding/quantized/` | GGUF量子化モデルローダー（Candle） |
| `crates/ene-ai-core/src/summarizer.rs` | LLMベースの会話要約 |
| `crates/ene-ai-core/src/conversation_manager.rs` | セッション境界関数（check_boundary、execute_split等） |
| `crates/ene-ai-core/src/special_token.rs` | emoトークン解析（感情表現） |
| `crates/ene-ai-core/src/mcp_client.rs` | MCPクライアント |
| `crates/ene-ai-core/src/tool_factory.rs` | ToolRegistryBuilder |
| `crates/ene-ai-core/src/memory/mod.rs` | メモリモジュール |
| `crates/ene-ai-core/src/memory/store/mod.rs` | SQLiteメモリストレージ（Diesel + sqlite-vec） |
| `crates/ene-ai-core/src/memory/store/models.rs` | Dieselモデル定義 |
| `crates/ene-ai-core/src/memory/recall.rs` | 要約フォーマット |
| `crates/ene-ai-core/src/sandbox/mod.rs` | サンドボックス設定 |
| `crates/ene-ai-core/src/sandbox/permission.rs` | 権限ゲート（PermissionGate、PermissionLevel） |
| `crates/ene-ai-core/src/tools/mod.rs` | ツールモジュール + ToolCategory列挙型 |
| `crates/ene-ai-core/src/tools/definition.rs` | ToolRegistryトレイト + ToolDefinition |
| `crates/ene-ai-core/src/tools/registry.rs` | OpencodeToolRegistry（統合ツールレジストリ） |
| `crates/ene-ai-core/src/tools/composite.rs` | 統合レジストリ |
| `crates/ene-ai-core/src/tools/builtin.rs` | 組み込みツール |
| `crates/ene-ai-core/src/tools/screenshot.rs` | スクリーンショットツール |
| `crates/ene-ai-core/src/tools/undo_manager.rs` | Undoシステム |
| `crates/ene-ai-core/src/tools/undo_tool/mod.rs` | Undoツール |
| `crates/ene-ai-core/src/tools/filesystem/mod.rs` | 統一ファイルシステムツール（read/write/edit/delete/glob/grep/patch） |
| `crates/ene-ai-core/src/tools/read/mod.rs` | 読み込み実装（offset/limit/binary対応） |
| `crates/ene-ai-core/src/tools/write/mod.rs` | 書き込み実装 |
| `crates/ene-ai-core/src/tools/edit/mod.rs` | 編集ツール（9段階置換戦略） |
| `crates/ene-ai-core/src/tools/delete/mod.rs` | 削除実装 |
| `crates/ene-ai-core/src/tools/search/mod.rs` | glob/grep検索 |
| `crates/ene-ai-core/src/tools/patch/mod.rs` | パッチ適用 |
| `crates/ene-ai-core/src/tools/shell/mod.rs` | シェル実行ツール |
| `crates/ene-ai-core/src/tools/shell/platform.rs` | プラットフォーム固有シェル実装 |
| `crates/ene-ai-core/src/tools/browser/mod.rs` | ブラウザ自動化ツール（CDP） |
| `crates/ene-ai-core/src/tools/browser/chrome.rs` | Chrome実行ファイル検出 |
| `crates/ene-ai-core/src/tools/browser/session.rs` | ブラウザセッション管理 |
| `crates/ene-ai-core/src/tools/browser/extract.rs` | DOM抽出・変換 |
| `crates/ene-ai-core/src/tools/app/mod.rs` | GUI自動化ツール（enigo/xcap） |
| `crates/ene-ai-core/src/tools/app/actions.rs` | GUI操作実装 |
| `crates/ene-ai-core/src/tools/webfetch/mod.rs` | Webフェッチツール |
| `crates/ene-ai-core/src/tools/webfetch/converter.rs` | コンテンツ変換 |
| `crates/ene-ai-core/src/tools/websearch/mod.rs` | ウェブ検索ツール |
| `crates/ene-ai-core/src/tools/websearch/backends.rs` | 検索バックエンド（DuckDuckGo/Tavily/Brave） |
| `crates/ene-ai-core/src/tools/todo/mod.rs` | Todo管理 |
| `crates/ene-ai-core/src/tools/todo/store.rs` | Todoストア |
| `crates/ene-ai-core/src/tools/question/mod.rs` | 質問ツール |
| `crates/ene-ai-core/src/tools/truncate/mod.rs` | 出力切り詰めユーティリティ |
| `crates/ene-app/src/main.rs` | GUIエントリーポイント |
| `crates/ene-app/src/app_config.rs` | アプリ設定（CharacterSettings） |
| `crates/ene-app/src/character.rs` | キャラクタープラグイン |
| `crates/ene-app/src/ai_bridge.rs` | Bevy AI統合 |
| `crates/ene-app/src/resources.rs` | Bevyリソース |
| `crates/ene-app/src/scene.rs` | シーン管理 |
| `crates/ene-app/src/tray.rs` | システムトレイ |
| `crates/ene-app/src/settings_ui/` | 設定UIウィジェット |
| `crates/ene-app/src/window_drag/` | ウィンドウドラッグ（Linux/Windows） |
| `crates/ene-app/src/platform.rs` | プラットフォーム判定 |
| `crates/ene-cli/src/main.rs` | CLIエントリーポイント |
| `crates/ene-cli/src/cli.rs` | CLI引数解析 |
| `crates/ene-cli/src/config.rs` | CLI設定 |
| `crates/ene-cli/src/context.rs` | CLIコンテキスト |
| `crates/ene-cli/src/repl.rs` | REPLループ |
| `crates/ene-cli/src/stream.rs` | CLIストリーム処理 |
| `crates/ene-cli/src/registry.rs` | CLIツールレジストリ |
| `crates/ene-cli/src/tooltest.rs` | ツールテスト |
| `crates/ene-cli/src/commands/` | CLIコマンド（session/memory） |
| `crates/ene-cli/src/style.rs` | スタイル定義 |
