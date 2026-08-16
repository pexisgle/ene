# 移行計画(現行コード → 新構成)

> 実装ウェーブの順序・完了条件・リスクを定義する。
> 各文書が「何を」作るか、この文書が「どの順で」作るかを決める。

## 1. 現行クレート → 新構成のマッピング

現行コードは**参照**であり、制約ではない(vision.md 設計原則7)。
判定は **流用**(ほぼそのまま)/ **書き直し**(概念は活かし再実装)/
**破棄**(新設計で不要)の3種。

### crates/

| 現行 | 判定 | 行き先 | 根拠 |
|---|---|---|---|
| `ene-core` | 書き直し | ドメイン語彙は新カーネルへ。MemoryPort の考え方は memory.md に継承 | 語彙は使えるが、イベントソーシング中心に再編 |
| `ene-config` | 流用 | 設定システム(process-model.md §4)の基盤 | defaults→JSON→env+スキーマ自動生成はそのまま使える |
| `ene-card` | 破棄(読み込み部のみ流用) | V3 インポートは character/package-format.md §9 へ | V3 を正規格から外す(確定済み) |
| `ene-mind` | 書き直し | companion/(soul-affect・memory・proactive・inner) | パイプライン概念は活きるが、レーン/内面/秘匿の再編が必要 |
| `ene-store` | 書き直し | session-log.md・storage-model.md・memory.md のスキーマへ | SeaORM 依存の見直しを含め、SQLite 単一ソース設計に再編。3ストア/トランザクション規律は新規実装 |
| `ene-ai` | 書き直し | プロバイダ seam(capabilities.md §8)+プラグインプロバイダへ | プロバイダはアウトプロセス化。ホスト側は seam のみに |
| `ene-rag` | 流用(縮小) | memory.md の検索・スコアリング部 | 純粋ポリシーは新記憶系に組み込める |
| `ene-runtime` | 書き直し | コアデーモン+ループ(process-model.md・agent-loop.md・operations.md・durability.md) | アクター/catch_unwind 方針は継承。単一フライト前提は破棄(レーン化)。操作状態機械と回復は新規実装 |
| `ene-approval` | 書き直し | security/approval.md | ポリシーDSL・AI自動承認・監査の追加 |
| `ene-artifact` | 書き直し | tasks/jobs-and-schedules.md §5(CAS の考え方は継承) | 成果物体系への統合 |
| `ene-connector` | 流用 | capabilities.md §6 の connector seam | 構造が使える。credential はボールト経由へ |
| `ene-infer` | 流用 | ローカル推論のプロバイダプラグイン内へ | 単一スレッド機構はプラグイン内で活かす |
| `ene-sandbox` | 流用 | security/sandbox.md の基盤 | Landlock/bwrap 実装は新 IPC に合わせて整理 |
| `ene-plugin-proto` | 書き直し | plugins/ipc.md(MessagePack+FD 世代へ) | ワイヤ ABI の再定義。v7 の経験は活かす |
| `ene-plugin` / `-macros` | 書き直し | プラグイン SDK(新 IPC 向け) | 作者向けファサードは再設計 |
| `ene-plugin-host` | 書き直し | プラグイン監督(process-model.md §3) | MCP クライアント含む監督へ |
| `ene-plugin-broker` | 流用 | plugins/broker.md | Broker の概念・実装は拡張して継承 |
| `ene-plugin-db` | 破棄 | —(プラグインの DB 直接アクセスは新設計で廃止) | 資源は Broker 仲介のみ |
| `ene-voice` | 流用 | body/voice.md のローカルエンジン部 | Whisper/Kokoro/Silero 系はプラグイン化して継承 |
| `ene-vrm` | 流用 | body-and-performance.md の VRM 描画 | 描画専用クレートの方針は維持 |
| `ene-util` | 流用 | 各所の純粋ユーティリティ | そのまま |

### apps/

| 現行 | 判定 | 行き先 |
|---|---|---|
| `ene-desktop` | 書き直し | platform/clients.md の desktop(stage)。コアの起動+描画+音声中継に責務を絞る |
| `ene-cli` | 書き直し | platform/clients.md の CLI。`ene-core` の起動と操作のクライアントへ |
| (新規) | — | `ene-core`(コアデーモン)を追加 |

### plugins/

| 現行 | 判定 | 行き先 |
|---|---|---|
| `plugins/tool/*`(fs/web/browser/git/app/calendar 等) | 書き直し(資産活用) | 新 IPC のツールプラグインへ。実装ロジックは流用し、線と manifest を新形式に |
| `plugins/provider/*`(openai/anthropic/local-llm 等) | 書き直し(資産活用) | 新 IPC のプロバイダプラグインへ |

## 2. 実装ウェーブ

各ウェーブは「完了条件」を満たして次へ進む。並行できるものは並行する。

### W0: 基盤(カーネルの骨)

- session-log のイベント語彙+SQLite 永続化+投影(L-1..L-5)。
- storage-model の3ストア+トランザクション規律(BEGIN IMMEDIATE・
  レジスタ名前空間・usage 台帳)。
- agent-loop の最小(対話レーンのみ、単一フライト)+ operations の
  `op.state` プログラムカウンタ+durability の effect sandwich。
- lane-api のコマンド面(受理/拒否・結果・`lane.last_result` 照合)。
- visibility の出力秘匿モデル(thinking の分離・投影の要約形・
  投影走査テストの骨格)。
- observability の最小(スパン語彙・ローカルリング・コンテンツ禁止)。
- context-assembly の最小(Context Source・epoch なしの組立)。
- コアデーモンの起動・設定・アクター配置。
- **完了条件**: テキストのターンが回り、ログに記録され、投影から
  履歴が再現できる。`model-visible = logged` の検証テストが通る。
  ターン実行中の任意のクラッシュ位置からの回復が中断なし実行と一致する
  (invariants.md 階層 A のサブセット)。

### W1: 実行系(ツールとプラグイン)

- plugins/ipc.md の MessagePack+FD 線。manifest とプロファイル。
- ツールレジストリ+パイプライン。ビルトインツールの最小(fs/exec/web)。
- Broker の最小(fs/net)。サンドボックス(Landlock)の適用。
- **完了条件**: アウトプロセスのツールが登録・実行され、
  サンドボックス内で動く。IPC オーバーヘッドが予算内(5ms 中央値)。

### W2: セキュリティ面

- 承認 plane(ポリシーDSL+ポップアップ+監査ログ)。
- 資格情報ボールト。AI 自動承認。対話からのポリシー追加。
- **完了条件**: approval.md の受入条件(P-903..P-906)が観察できる。
  監査ログの hash chain が検証できる。

### W3: コンパニオン層

- soul-and-affect(ハイブリッド感情)。inner-channel。memory 全体。
- proactive。character/package-format の実装(+V3 インポート)。
- **完了条件**: 再起動跨ぎの記憶・感情の一貫性・内面表示・
  能動発話・キャラパッケージのインストールが動く(P-2xx/P-3xx/P-8xx)。

### W4: 身体と音声

- body-and-performance(VRM 描画+キュー)。voice(全二重)。
- **完了条件**: 全二重音声のバグイン・自声回避・リップシンクが動く
  (P-102/P-103/P-4xx)。

### W5: 仕事面

- delegation.md の非同期委譲(public/internal 両モード)・親↔子
  メールボックス・報告ターン。job レーン・スケジュール・ワークフロー・
  artifact。skills。MCP seam。connector の最小(calendar/mail)。
- **完了条件**: 対話と並行して job が走り、成果物が交付される。
  リマインダーが鳴る。MCP ツールが動く(P-6xx)。
  会話をしながら委譲の完了報告が届き、ユーザー発話が優先される。
  internal 委譲の秘匿検証テスト(visibility.md §6)が通る。

### W6: プラットフォーム完成

- server-api の全体(HTTP/WS/SDK 生成)。clients の desktop/CLI/Web。
- server-api は lane-api の射影として構築(エンドポイント=コマンド対応表、
  [../core/lane-api.md](core/lane-api.md) §5)。
- 複数クライアントの排他資源調停。バックアップ。
- **完了条件**: 3クライアントが同一コアに接続し、同時接続・
  排他調停が動く(P-7xx)。性能予算を満たす。

### W7: 統合と v1.0 判定

- product/done.md の全受入条件の検証。性能ベンチ。オフライン検証。
- **完了条件**: done.md の総括条件1..5 が同時に成立する。

## 3. リスク一覧

| リスク | 影響 | 緩和 |
|---|---|---|
| 全アウトプロセスによる軽量化の困難 | 性能予算の未達 | MessagePack+FD のゼロコピー寄り設計(W1 で早期にベンチ)。予算超過時は IPC の最適化に集中 |
| 感情ハイブリッドの不自然さ | コンパニオン品質 | 裁定器のヒステリシス調整。離散ボキャブラリの centroid をパッケージ側で調整可能に |
| V3 インポートの情報欠落 | インポート品質 | 汎用 emotion_map を既定で付与。欠落は警告のみ(拒否しない) |
| イベントソーシングの性能 | セッション長大化 | 索引の最小化+PK 順走査。compaction で履歴を有界に |
| MCP の実装差分 | 相互運用 | 最小の stdio/Streamable HTTP のみサポートし、ツール面を先に安定させる |
| 秘匿サブエージェントのデバッグ困難 | 開発効率 | 子セッションの永続ログ(診断専用)+CLI の debug コマンド(開発者のみ) |
| 非同期委譲の報告がユーザーに届かない/遅れる感覚 | 体験品質 | 報告は pending.entry で失われない。結合配信+`delegation.active` Source で親が境界で現状を知る。遅延は対話で説明できる(「まだやってるよ」) |
| thinking 秘匿による「考えている様子」の喪失 | コンパニオン品質 | 内面派生(visibility.md §4)。発話+内面+仕草で思考の存在感を補う |
| 移行中の並行開発の衝突 | 作業効率 | ウェーブ単位で完了条件を固定し、並行は別フォルダ/別クレートに限定 |

## 4. やらないこと(移行期)

- 現行の `docs/` と AGENTS.md は、新構成が確定するまで書き換えない。
- 現行の機能停止を伴う「ビッグバン置換」はしない。ウェーブごとに
  新構成を増やし、最後に切替える。
- データ移行ツールは v1.0 の範囲外(未リリースのため、
  既存データの互換性は保証しない)。

---

- 前: [security/README.md](security/README.md) / 次: [README.md](README.md)(目次へ)
