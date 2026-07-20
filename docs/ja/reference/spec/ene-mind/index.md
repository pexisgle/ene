# `ene-mind` クレート概要 & モジュール構成

`ene-mind` クレートは、AIキャラクタープラットフォーム Ene の中核となる「認知エンジン (Cognitive Mind)」を実装します。会話履歴や設定、データベース内のベクトル記憶を分析し、LLMに投入するための最適化されたコンテキスト（プロンプト）の選定・組み立てを行い、感情や表情、キャラクターのアイデンティティ（カード仕様）を管理します。

---

## 1. 依存関係と境界

### 物理的依存関係 (`Cargo.toml`)
- **依存先**: `tokio`, `serde`, `serde_json`, `chrono`, `tracing`, `async-trait`, `regex`, `parking_lot`
- **内部クレート依存先**: `ene-ai`, `ene-config`, `ene-store`
- **禁止されている依存**: `ene-runtime` および `ene-tool-host` への依存は禁止されています。脳（`ene-mind`）は自身がどのような非同期スレッドアクター（`ene-runtime`）やツール実行プロセス（`ene-tool-host`）で動作しているかを知ってはならず、純粋なステートマシンとしてのインターフェースのみを提供します。

### 論理的境界
- **`ene-store` との通信**: 長期記憶や感情、コミットメントをロード・セーブする際、`ene-store` の `MemoryStore` トレイト（または具象型）の公開メソッドのみを経由し、自身で生の SQL や `sea-orm` クエリを発行することはありません。

---

## 2. モジュール構造

```text
ene-mind/src/
├── lib.rs              # クレートルート。各種インターフェースの再エクスポート
├── config.rs           # 認知エンジンの詳細設定構造体 (MindConfig)
├── engine.rs           # 認知全体をオーケストレーションする CognitionEngine
├── error.rs            # 認知系エラー (CognitionError)
├── lifecycle.rs        # ターン実行用の入力・出力データオブジェクト (PreTurnOutputなど)
├── summarizer.rs       # 会話セッション分割時の要約LLMユーティリティ
├── character/          # キャラクターアイデンティティ・Lorebook のコンパイル
├── commitments/        # 約束・タスク台帳 (CommitmentLedger)
├── context/            # コンテキストトークン予算および圧縮管理
├── emotion/            # 感情エンジン（PADモデル・LLM査定）
├── memory_journal.rs   # ログ調査用 scored 記憶検索ファサード
├── memory_writer/      # 記憶抽出（MemoryArbiter）・忘却減衰モデル
├── output/             # 表情・モーション出力調停 (OutputArbiter)
├── pre_turn/           # 前ターン入力分析
├── proactive/          # 能動話話・スクリーン画像解釈ロジック
├── prompt_packet/      # 予算に応じた物理プロンプトパケット構成
├── recall/             # 長期記憶の回収・ハイブリッド検索プランナー
└── session/            # セッション定義、CharacterCardV3、表情定義
```

---

## 3. 主要モジュール概要リンク

詳細な関数・構造体レベルの仕様は以下に分割されています。

*   [CognitionEngine & ターンライフサイクル](engine.md)
*   [RecallPlanner & ハイブリッド長期記憶検索](recall.md)
*   [MemoryArbiter / 長期記憶自動抽出 & 減衰忘却](memory_writer.md)
*   [EmotionEngine / PADモデル感情状態](emotion.md)
*   [ContextManager / セッション圧縮分割・トークン予算](context.md)
*   [ConversationSession / キャラクターカードCBSマクロ](session.md)
*   [Proactive Speech / 能動話話判断 & ビジョン解釈](proactive.md)
