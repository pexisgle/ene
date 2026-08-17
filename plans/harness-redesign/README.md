# Ene ハーネス再設計 — 設計文書群

Ene を「コーディング特化ではない AI コンパニオン型エージェントハーネス」へ
再設計する文書群。2層構造: **プロダクト定義(完成形)が先、設計は後**。

1. **[product/](product/README.md)** が「完成形」を定義する。
2. 領域別フォルダが、`product/features.md` の要件 ID(`P-xxx`)を
   実現する設計を記述する。各文書は冒頭に「実現する P-xxx」を置く。

## 目次と読み順

| # | 文書/フォルダ | 内容 |
|---|---|---|
| 0 | [product/](product/README.md) | ビジョン・機能目録(P-xxx)・完成の定義・用語集 |
| 1 | [core/](core/README.md) | ハーネスカーネル(11文書): 語彙層(ログ・組立・ループ・委譲・出力秘匿)+ 耐久性層(ストレージ・操作状態機械・回復・不変条件)+ 駆動/診断面(レーンコマンド・可観測性) |
| 2 | [plugins/](plugins/README.md) | プラグイン実行系: IPC・manifest/プロファイル・Broker |
| 3 | [tools/](tools/README.md) | ツール: 統一レジストリ・capability seam・MCP |
| 4 | [companion/](companion/README.md) | コンパニオン層: soul/感情・内面・記憶・能動発話 |
| 5 | [body/](body/README.md) | 身体と声: パフォーマンスプロトコル・全二重音声 |
| 6 | [character/](character/README.md) | キャラパッケージ形式(soul/body/char) |
| 7 | [tasks/](tasks/README.md) | 仕事面: job・スケジュール・成果物・skill |
| 8 | [platform/](platform/README.md) | プラットフォーム: プロセスモデル・API・クライアント |
| 9 | [security/](security/README.md) | 安全: サンドボックス・承認 plane・監査 |
| 10 | [migration.md](migration.md) | 現行コードからのマッピング・実装ウェーブ・リスク |

初回は `product/` を読み、以降は上の順で読むのが最短経路。
用語は [product/vision.md の用語集](product/vision.md#用語集)に厳密に定義される。

## P ↔ 文書 対応早見表

| P グループ | 主題 | 主な担当文書 |
|---|---|---|
| P-1xx | 会話と存在感 | core/agent-loop, body/voice, companion/proactive, companion/inner-channel |
| P-2xx | 記憶と人格 | companion/memory |
| P-3xx | 感情 | companion/soul-and-affect, body/body-and-performance |
| P-4xx | 身体と表示 | body/body-and-performance, character/package-format |
| P-5xx | ハーネス | core/(語彙層5 + 耐久性層4 + 駆動/診断面2 の計11文書)、表層/裏層は product/vision |
| P-6xx | ツールと仕事面 | tools/(2文書), tasks/(2文書) |
| P-7xx | プラットフォーム | platform/(3文書) |
| P-8xx | キャラパッケージ | character/package-format |
| P-9xx | 安全とプライバシー | security/(2文書), plugins/broker |
| P-10xx | 拡張性 | plugins/(2文書), platform/process-model |

## 確定済みの骨格決定(全文書の前提)

1. 純 Rust 軽量カーネル。プラグインは**全アウトプロセス**
   (Unix socket / named pipe + 長さプレフィックス MessagePack + FD 渡し)。
2. コアデーモン+マルチクライアント(desktop 主・CLI デバッグ・Web 外部接続)。
3. SQLite 単一ソースのイベントソーシング・セッションログ
   (`model-visible = logged` 不変条件)。
4. コンパニオンごとに**表層 soul + 裏層ハーネス**の2層。表層がユーザー
   入力の唯一の入口(対話レーン1本)、裏層が作業(独立 job レーン)。
   両層は Codex multi-agent v2 風の層間メッセージで進捗を共有し、
   ユーザーからは1体に見える(P-522)。
5. 複数コンパニオン並存。メモリ非共有が既定、共有は共有スペースのみ。
   サブエージェントはユーザーから完全秘匿。
6. 構成可能プラグインツリー(プロファイル/バンドル/パッチ)。
7. 統一ツールレジストリ。Tool Calling(モデル規約)と MCP(供給元規約)は
   別レイヤーとして統合。
8. 承認は事前ポリシー+AI 自動承認+ポップアップ。対話からのポリシー追加可。
9. soul/body は分離。既定は「セット=1キャラ」、自由な結合を公式サポート。
10. 感情はハイブリッド(離散ボキャブラリの対外層+PAD の内部状態層)。
11. 内面チャネルは永続+自己参照窓。
12. 音声は全二重・サーバー所有。
13. workspace は内部概念のみ。**spawn(委譲)ごと**に独立し、fs/exec の既定スコープになる。成果物は広範+段階化。skill は業界標準準拠。
14. すべての LLM 呼び出しでクラウド/ローカルを選択可能。会話はクラウド推奨。
15. Broker 仲介原則(manifest 同梱アセットは素通り)。
16. ハーネスの耐久性: 永続状態は3ストア(entries/registers/usage)に
    尽き、ループの現在位置はレジスタ上のプログラムカウンタ。
    任意のクラッシュ位置から重複効果なしで再開できる(P-515)。
17. **出力秘匿モデル**: LLM の生出力(thinking・ツール引数・生出し・
    子セッション)はユーザーに見せないのが既定。見えるのは発話・
    内面(オプトイン)・要約・成果物(P-520)。thinking は記録するが表示しない。
18. 実作業は**非同期委譲**に移す。表層は待たず会話を続け、完了報告は
    対話レーンにターンとして届く(P-519/P-521)。job と秘匿サブエージェントは
    同機構の public/internal モード。表層は副作用のある作業ツールを持たない。
19. ホストはコアデーモンただ1つ。表層と裏層はホストを割らず、同一
    ハーネスカーネル(ログ・耐久性・Tool Calling / MCP / skill)の上の
    **専用実装**として走る。公開ツール面のフィルタだけではない。

## この文書群の使い方

- 実装は [migration.md](migration.md) のウェーブ順で進める。
- 設計文書と product/ が矛盾したら **product/ が勝つ**。
- P-xxx にない機能を設計に足さない。足すならまず
  [product/features.md](product/features.md) に ID を追加する(要レビュー)。
- 「実装が一意に定まる」粒度(イベント語彙・スキーマフィールド・
  状態遷移・不変条件・設定キーと既定値)で書いてある。コードそのものは
  書かない(実装者の言語選択の余地は残さないが、コード表現は実装に委ねる)。
