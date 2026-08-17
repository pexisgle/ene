# 機能目録

> プロダクトが持つ機能を要件 ID(`P-xxx`)付きで列挙する唯一の文書。
> 各設計文書は冒頭に「実現する P-xxx」を置く。
> 位置づけと設計原則は [vision.md](vision.md)、完成の定義は [done.md](done.md)。

## 凡例

- **優先度**: `v1.0` = v1.0 の完了に必須。`stretch` = v1.0 に含めるが、完了判定から
  除外可能な拡張([done.md](done.md)で stretch の扱いを定義)。`後継` = v1.0 の完了定義の外。
  ただし後継でも**形式が支える**(後継が既存形式の破壊的変更を強制しない)。
- **対応サブシステム**: 設計文書のフォルダ/ファイル。

---

## P-1xx 会話と存在感

| ID | 機能 | 説明 | 優先度 | 対応サブシステム |
|---|---|---|---|---|
| P-101 | テキスト対話 | テキスト入力は表層 soul の対話レーンでターンを起こし、ストリーミング応答を得る。複数クライアントから同一セッションに同時接続可能。裏層ハーネスへは直接入らない。 | v1.0 | core/agent-loop, platform/server-api |
| P-102 | 全二重音声対話 | 話しかけによる割り込み(バグイン)、自声回避、中断からの再開を含む双方向音声。パイプラインはコアデーモン所有。音声もテキストと同様、表層 soul が唯一の入口。 | v1.0 | body/voice |
| P-103 | 割り込みとキャンセル | 発話中・生成中をユーザー発話で中断できる。中断されたターンは履歴に「中断」として記録される。 | v1.0 | core/agent-loop, body/voice |
| P-104 | 内面の表示 | 思考・感情表出・行動意図を発話と分離して表示(内面チャネル)。ユーザーは内面表示を off にできる。 | v1.0 | companion/inner-channel |
| P-105 | 能動発話 | ゲート(クールダウン・quiet hours・疲労)を通過したとき、コンパニオンが自発的に話す。完全無効化可能。 | v1.0 | companion/proactive |
| P-106 | 静寂の同居 | 何も話さない状態を「故障」ではなく「寄り添い」として扱う。idle の状態表示と最小限の反応(視線・軽い仕草)がある。 | v1.0 | companion/proactive, body/body-and-performance |
| P-107 | 複数コンパニオンの並行会話 | 各コンパニオンが独立セッションで同時に会話できる。表層の対話レーンは soul ごとに独立。 | v1.0 | core/agent-loop, platform/clients |
| P-108 | セッションの分割・一覧・再開 | セッションは明示分割・アイドルタイムアウト・話題境界で終わり、要約が残る。一覧・検索・アーカイブ・再開が可能。 | v1.0 | core/session-log |
| P-109 | 会話の fork と分岐の保持 | セッションログの任意時点から fork できる。元は保持される。 | v1.0 | core/session-log |
| P-110 | 会話のエクスポート | セッションを秘匿処理(資格情報・内面)済みの JSON/Markdown で書き出せる。 | v1.0 | core/session-log |
| P-111 | 音声の翻訳再生 | テキストはユーザー言語、音声は別言語で再生する(TTS 翻訳)。 | 後継 | body/voice |
| P-112 | 視覚認知 | カメラ・画面共有・スクリーンショットからコンパニオンが状況を見る。 | 後継 | tools/capabilities(vision seam) |
| P-113 | タッチ・クリック反応 | アバターへのクリック・ドラッグに反応する。 | 後継 | body/body-and-performance |
| P-114 | 同室会話(複数体の1セッション) | 複数コンパニオンが同一セッションに参加し、発話権を調停して会話する。 | 後継 | core/session-log(room 拡張) |

## P-2xx 記憶と人格

| ID | 機能 | 説明 | 優先度 | 対応サブシステム |
|---|---|---|---|---|
| P-201 | soul 専用メモリボールト | コンパニオンごとの永続メモリ。再起動・セッション跨ぎで保持。他 soul から読めない。 | v1.0 | companion/memory |
| P-202 | 記憶の種別体系 | エピソード(出来事)・意味(事実)・ユーザープロファイル・好み・約束(commitments)の種別。 | v1.0 | companion/memory |
| P-203 | ハイブリッド検索 | ベクトル+語彙+recency+関係性+減衰の複合スコアで記憶を想起。 | v1.0 | companion/memory |
| P-204 | ターン後の記憶抽出 | ターン終了後にバックグラウンドで記憶候補を抽出(決定論+LLM)。対話を止めない。 | v1.0 | companion/memory |
| P-205 | 記憶の裁定 | 重複・矛盾・重要度による採用判断。矛盾時は新記憶が旧記憶を置換し、置換はログに残る。 | v1.0 | companion/memory |
| P-206 | 忘却と減衰 | 使われない記憶は減衰し、閾値以下で忘却候補になる。完全削除前にユーザー確認または承認キュー。 | v1.0 | companion/memory |
| P-207 | 共有スペース | 命名メモリ領域を複数の soul がオプトインで共有。権限(読み/書き)はスペースごとに設定。 | v1.0 | companion/memory |
| P-208 | ユーザープロファイル | ユーザーの呼称・好み・予定傾向などの長期プロファイルを対話から学習・編集可能。 | v1.0 | companion/memory |
| P-209 | 関係性の蓄積 | soul ごとにユーザーとの関係メトリクス(信頼・親密度)が蓄積し、人格と応答に影響する。 | v1.0 | companion/soul-and-affect |
| P-210 | 記憶のユーザー可視化と編集 | ユーザーが「覚えていること」を閲覧・編集・削除できる。削除はセッションログにも記録される。 | v1.0 | companion/memory, platform/server-api |
| P-211 | 記憶の承認キュー | 自動採用が不適切な記憶候補(機密・第三者情報など)を承認キューに回す。 | v1.0 | companion/memory, security/approval |
| P-212 | soul 間の記憶の贈り物 | soul が明示的に「メモを渡す」ツールで共有スペースへ書き込む。 | v1.0 | companion/memory |
| P-213 | 内省(reflection) | 定期的な自己内省で記憶を整理・再構成する。 | stretch | companion/memory |
| P-214 | 記憶の完全ローカル暗号化 | メモリ DB の保存時暗号化(ユーザーパスフレーズ)。 | 後継 | security/approval |

## P-3xx 感情

| ID | 機能 | 説明 | 優先度 | 対応サブシステム |
|---|---|---|---|---|
| P-301 | ハイブリッド感情モデル | 離散感情ボキャブラリ(対外インターフェース)+ PAD 連続空間(内部状態)の2層。 | v1.0 | companion/soul-and-affect |
| P-302 | 感情の減衰と持続 | 内部感情状態は時間経過でカードのベースラインへ減衰する。関係メトリクスはゆっくり動く。 | v1.0 | companion/soul-and-affect |
| P-303 | 感情→表情/モーション写像 | 離散感情と強度を body の表情・モーションへ写像。写像表は body パッケージ側が持ち、soul 結合時に検証する。 | v1.0 | body/body-and-performance |
| P-304 | 感情の出力裁定 | 感情表出キューの検証・ヒステリシス・レート制限。表情のちらつきを防ぐ。 | v1.0 | companion/soul-and-affect |
| P-305 | 感情の自己報告 | モデルが内面チャネルで感情を報告でき、それが PAD 状態への入力イベントになる。 | v1.0 | companion/inner-channel, companion/soul-and-affect |
| P-306 | 疲労と quiet hours | 疲労・時間帯ゲートが能動発話と感情表出の頻度を抑制する。 | v1.0 | companion/proactive |
| P-307 | 感情のユーザー可視化 | 感情状態をユーザーが粗い形で確認できる(任意)。 | stretch | companion/soul-and-affect |
| P-308 | 感情ベースラインの編集 | キャラクター定義の感情ベースラインをユーザーが調整できる。 | stretch | character/package-format |

## P-4xx 身体と表示

| ID | 機能 | 説明 | 優先度 | 対応サブシステム |
|---|---|---|---|---|
| P-401 | soul/body 分離 | soul と body は独立エンティティ。1 soul は任意の互換 body に結び付く。 | v1.0 | body/body-and-performance, character/package-format |
| P-402 | 自由な結合 | ユーザーが soul と body を自由に組み替えられる(公式機能)。互換性検証と警告がある。 | v1.0 | character/package-format |
| P-403 | VRM 表示 | VRM 1.0 アバターをネイティブ(wgpu)で描画。desktop stage の既定 body 形式。 | v1.0 | body/body-and-performance |
| P-404 | 表情・モーション・リップシンク・視線 | パフォーマンスプロトコルに基づくキューで身体を動かす。リップシンクは音声出力と同期。 | v1.0 | body/body-and-performance |
| P-405 | 複数コンパニオンの同時表示 | stage に複数体を並べて表示する(各自独立 body 制御)。 | stretch | body/body-and-performance, platform/clients |
| P-406 | body の hot swap | セッション中に body を差し替えても会話は途切れない。 | v1.0 | body/body-and-performance |
| P-407 | テキスト専用モード | 描画なし(音声・テキストのみ)で動作する軽量モード。 | v1.0 | body/body-and-performance |
| P-408 | Live2D/2D 形式の body | Live2D・静止画・テキストアバターを body 形式として追加。 | 後継 | body/body-and-performance |
| P-409 | デスクトップペットモード | 透過・最前面・クリックスルーでデスクトップ上に常駐。 | 後継 | platform/clients |

## P-5xx エージェントハーネス

| ID | 機能 | 説明 | 優先度 | 対応サブシステム |
|---|---|---|---|---|
| P-501 | イベントソーシング・セッションログ | セッションの全事実を追記専用ログに記録。SQLite 単一ソース。 | v1.0 | core/session-log |
| P-502 | model-visible = logged | モデルに見えるものは必ずログから再構成可能。検証手段を持つ。 | v1.0 | core/session-log |
| P-503 | turn/step ループ | ターンは0以上のステップから成り、各ステップは1回のモデル要求+ツール実行。 | v1.0 | core/agent-loop |
| P-504 | 対話1 + job 並行 | コンパニオンごとに表層の対話レーン(単一フライト)と、裏層の独立した job レーンを持つ。 | v1.0 | core/agent-loop |
| P-505 | コンテキスト組立 | Context Source レジストリから System Context を組み立て、Context Epoch を管理。 | v1.0 | core/context-assembly |
| P-506 | compaction | 履歴が窓を溢れる前に要約へ畳む。ユーザーには「忘れないための整理」として最小限通知。 | v1.0 | core/context-assembly |
| P-507 | spill | 巨大ツール出力をファイルへ逃がし、履歴には要約+参照。 | v1.0 | core/context-assembly |
| P-508 | 秘匿サブエージェント | 裏層が内部で委譲実行するサブエージェント。ユーザーのイベント・UI から完全に秘匿。実体は非同期委譲(P-519)の internal モードであり、job は同機構の public モード。 | v1.0 | core/delegation |
| P-509 | guard | 繰り返し呼び出し検知・実行期限などのループ健全性ガード。 | v1.0 | core/agent-loop |
| P-510 | エラー回復 | モデル/ツール/ストリームの失敗分類と再試行・打ち切りの方針。対話的な報告。 | v1.0 | core/agent-loop |
| P-511 | plan モード | 変更を伴う作業前に計画を提示し、ユーザー承認を待つ協調面。 | v1.0 | core/agent-loop |
| P-512 | ask-user | ツール実行中にユーザーへ質問して回答を得る(ターンを中断しない)。 | v1.0 | core/agent-loop |
| P-513 | セッション横断の goal | 「この目標に向かって進める」状態をセッション跨ぎで保持する。 | 後継 | core/agent-loop |
| P-514 | エージェント実装の差し替え | Agent 抽象の実装差し替え(表層 soul / 裏層ハーネスそれぞれに別アーキテクチャを積む)。 | 後継 | core/agent-loop |
| P-515 | クラッシュ回復 | プロセスが任意の瞬間に死んでも、セッションは重複する外部効果なしに最後にコミットされた状態から再開する。正常終了(close)は制御されたクラッシュとして同一の回復経路を使う。 | v1.0 | core/durability, core/operations |
| P-516 | 使用量台帳 | 全 LLM 呼び出し(失敗・コンパクション含む)のトークン数とコストを追記専用台帳に記録し、セッション/soul/タスク/期間単位で集計・公開する。 | v1.0 | core/storage-model |
| P-517 | ローカル可観測性 | ターン/ステップ/ツール/コミット/プロバイダ要求の計測(スパン)をローカルに限定して備える。コンテンツは決して載せない。開発 CLI から閲覧できる。外部送信はしない(P-910)。 | v1.0 | core/observability |
| P-518 | スキーマ進化 | ストレージの永続形式にバージョンを持ち、アプリ更新時に開く際の連鎖マイグレーションで追従する。開いている操作の状態を含む全レジスタの変換規則を、状態機械の変更と同じ変更で定める。 | v1.0 | core/storage-model |
| P-519 | 非同期委譲と層間対話 | 実作業の委譲は非同期。表層 soul は裏層ハーネスを待たずに対話を続ける。両層は Codex multi-agent v2 風の層間メッセージ(`task` / `message` / `question` / `answer` / `final` / `cancel`)で進捗を共有する。 | v1.0 | core/delegation, core/agent-loop |
| P-520 | 出力秘匿モデル | LLM の生出力(thinking・ツール引数・ツール生出力・子エージェントの内部ログ)はユーザーから秘匿するのが既定。ユーザーが見るのは発話・内面(オプトイン)・要約カード・成果物のみ。 | v1.0 | core/visibility |
| P-521 | 完了報告の対話化 | 裏層の完了報告は表層の対話レーンにターンとして届き、表層がユーザーへの伝え方(発話・内面・静黙)を判断する。報告が対話を遮ることはない。 | v1.0 | core/delegation |
| P-522 | 表層/裏層の2層ランタイム | ユーザー入力(音声・テキスト)は表層 soul のみが受ける。表層は副作用のある作業ツールを持たず、作業が必要と判断したときだけ裏層ハーネスへ託す。両層は同一ハーネスカーネル上の別プロファイルであり、ユーザーからは1体のコンパニオンとして見える。 | v1.0 | product/vision, core/agent-loop, core/delegation |

## P-6xx ツールと仕事面

| ID | 機能 | 説明 | 優先度 | 対応サブシステム |
|---|---|---|---|---|
| P-601 | 統一ツールレジストリ | 全供給元のツールを1つのレジストリに集約。モデルには層ごとにフィルタされたスキーマ面を公開(表層は作業ツールを見ない)。 | v1.0 | tools/registry |
| P-602 | ビルトインツール群 | fs(読み書き/検索)、exec(シェル)、web(検索/取得)、ブラウザ操作、計算、日時、メモ。 | v1.0 | tools/capabilities |
| P-603 | MCP 接続 | MCP サーバーのツールを同一パイプラインで実行。resources は Context Source に、prompts は skill 素材に変換。 | v1.0 | tools/capabilities |
| P-604 | コネクタ | メール・カレンダー・ファイル同期などの外部サービス接続。credential はボールト管理。 | v1.0(最小) + 後継(拡張) | tools/capabilities |
| P-605 | job(バックグラウンド作業) | 対話と並行して進む作業。実体は非同期委譲の public モード。進捗イベント・成果物・キャンセルを持つ。ユーザーには「おつかい」として見える。 | v1.0 | tasks/jobs-and-schedules, core/delegation |
| P-606 | スケジュール | cron 風の持続スケジュール。発火はターン(proactive/scheduled)を起こす。quiet hours を尊重。 | v1.0 | tasks/jobs-and-schedules |
| P-607 | リマインダー | 「X時に思い出させて」が約束(commitments)+スケジュールで実現される。 | v1.0 | tasks/jobs-and-schedules |
| P-608 | ワークフロー | 複数ステップの自律遂行(例: 調べて→まとめて→文書化)。ユーザーには1つの job として見える。 | v1.0 | tasks/jobs-and-schedules |
| P-609 | artifact 体系 | テキスト/Markdown/コード/CSV に加え、オフィス文書(docx/xlsx/pptx)・画像・PDF を成果物として生成・保管・交付。形式は段階的に追加。 | v1.0(テキスト系) + 後継(オフィス/画像) | tasks/jobs-and-schedules |
| P-610 | skill | 手順+参照文書+スクリプトのパッケージ。カタログから読み込み、ターンに注入。業界標準準拠+独自拡張。 | v1.0 | tasks/skills |
| P-611 | コード実行 | 隔離されたコード実行環境(スクリプト実行・データ処理)。 | v1.0(最小: パイプ経由) + 後継(サンドボックス強化) | tools/capabilities |
| P-612 | コーディング支援 | fs/exec/git によるコード作業。コーディング特化 UX は持たないが、ツールとしては対等に持つ。 | v1.0 | tools/capabilities |
| P-613 | ツール実行のユーザー可視化 | ツール呼び出しを「今何をしているか」の**要約**として表示(引数・生出力は秘匿、内部機構は表示しない)。承認要求もここに出る。 | v1.0 | tools/registry, security/approval, core/visibility |
| P-614 | 巨大出力の扱い | ツール出力上限・spill・Managed ファイル参照。 | v1.0 | core/context-assembly |

## P-7xx プラットフォーム

| ID | 機能 | 説明 | 優先度 | 対応サブシステム |
|---|---|---|---|---|
| P-701 | コアデーモン | 唯一のホストとして常駐。desktop が起動し、CLI で直接起動も可能。 | v1.0 | platform/process-model |
| P-702 | HTTP API + WS | REST 風エンドポイント+WebSocket イベント購読。ローカル認証。 | v1.0 | platform/server-api |
| P-703 | 生成 SDK | OpenAPI 仕様から Rust/TS SDK を生成し、desktop/Web が同一 SDK を使う。 | v1.0 | platform/server-api |
| P-704 | desktop(stage) | 主クライアント。コンパニオン表示・音声入出力・操作シェル・設定 UI。 | v1.0 | platform/clients |
| P-705 | CLI | 開発・デバッグ・動作確認用。desktop にのみ最適化される機能がないことの検証手段でもある。 | v1.0 | platform/clients |
| P-706 | Web クライアント | desktop がホストするコアに外部(同一 LAN・トンネル)から接続するクライアント。 | v1.0 | platform/clients |
| P-707 | 複数クライアント同時接続 | 同一セッションに複数クライアントが対等に接続。排他資源(音声入力・承認応答)のみ調停。 | v1.0 | platform/clients |
| P-708 | 設定システム | defaults → JSON → 環境変数の優先順。スキーマ自動生成。設定変更の多くは再起動不要。 | v1.0 | platform/process-model |
| P-709 | バックアップと復元 | メモリ DB・セッションログ・設定のバックアップ/復元。 | v1.0 | platform/process-model |
| P-710 | モバイルクライアント | スマホアプリ(ホストかクライアントかは後継で決定)。形式が支える。 | 後継 | platform/clients |
| P-711 | 他プロセスへの API 公開(オートメーション) | 外部オートメーションが API でセッションを駆動できる(自動化専用プロファイル)。 | 後継 | platform/server-api |

## P-8xx キャラパッケージと共有

| ID | 機能 | 説明 | 優先度 | 対応サブシステム |
|---|---|---|---|---|
| P-801 | Ene 正規パッケージ形式 | soul 定義・body 定義・マニフェスト・アセットを1パッケージにまとめた正規形式。 | v1.0 | character/package-format |
| P-802 | セット販売前提の構造 | 既定は「soul+body セット=1キャラ」。soul 単独・body 単独の流通も形式上可能。 | v1.0 | character/package-format |
| P-803 | インポート | V3(PNG/CHARX)カードは**読み込みのみ**でインポート。正規形式へ変換。 | v1.0 | character/package-format |
| P-804 | エクスポート | キャラをパッケージファイルとして書き出し。 | v1.0 | character/package-format |
| P-805 | ローカライズ | パッケージ内のユーザー向け文字列を en-US/ja で持てる。 | v1.0 | character/package-format |
| P-806 | 配布仕様 | マーケットの土台となる配布形式(メタデータ・バージョン・チェックサム・署名)。 | v1.0(形式) + 後継(配信基盤) | character/package-format |
| P-807 | マーケット配信基盤 | 一覧・ダウンロード・評価・更新を伴う配信プラットフォーム。 | 後継 | —(アプリ外) |
| P-808 | パッケージの検証 | インストール時のスキーマ検証・容量上限・危険フィールドの検査。 | v1.0 | character/package-format |

## P-9xx 安全とプライバシー

| ID | 機能 | 説明 | 優先度 | 対応サブシステム |
|---|---|---|---|---|
| P-901 | サンドボックス | プラグイン子プロセスを OS 強制サンドボックス(Landlock/bubblewrap/AppContainer)で隔離。 | v1.0 | security/sandbox |
| P-902 | Broker 仲介 | プラグインは原則としてファイル・ネットワーク・資格情報に直接アクセスできない。Broker RPC 経由のみ。manifest 同梱アセットは素通り。 | v1.0 | plugins/broker |
| P-903 | 承認ポリシー | 事前ポリシー(ツール名+対象のワイルドカード)+粗粒度モード(すべて確認/AI 自動承認/すべて承認)。 | v1.0 | security/approval |
| P-904 | AI 自動承認 | ポリシーが「AI 自動承認」のとき、承認判断をモデルに委ね、理由を監査ログに残す。 | v1.0 | security/approval |
| P-905 | ポップアップ承認 | ポリシーでカバーされない実行は承認 plane のポップアップで確認。どのクライアントも応答可能。 | v1.0 | security/approval, platform/clients |
| P-906 | 対話からのポリシー追加 | 「次から確認しないで」を対話で受け、承認ポリシーに反映(ユーザー確認付き)。 | v1.0 | security/approval |
| P-907 | 資格情報ボールト | API キー等を暗号化保管。プラグインには Broker 経由で必要最小限だけ注入。 | v1.0 | security/approval |
| P-908 | 監査ログ | ツール実行・承認判断・ポリシー変更を永続監査ログに記録。 | v1.0 | security/approval |
| P-909 | オフライン可 | ローカルモデル構成ではネットなしで全機能が動く。プロバイダ抽象はクラウド/ローカルで同一経路。 | v1.0 | tools/capabilities, plugins/manifest-and-profile |
| P-910 | プライバシーの既定 | テレメトリなし。ローカルファースト。エクスポートには秘匿処理。 | v1.0 | product 原則, core/session-log |

## P-10xx 拡張性

| ID | 機能 | 説明 | 優先度 | 対応サブシステム |
|---|---|---|---|---|
| P-1001 | 全アウトプロセスプラグイン | ツール・プロバイダ・MCP ブリッジはすべて子プロセス。IPC は MessagePack+FD 渡し。 | v1.0 | plugins/ipc |
| P-1002 | プロファイル/バンドル/パッチ | 起動するプラグインツリーをプロファイル=バンドル積層+パッチで構成。 | v1.0 | plugins/manifest-and-profile |
| P-1003 | プロバイダ seam | LLM/埋め込み/STT/TTS/VAD のプロバイダを差し替え可能に。クラウド/ローカルを問わない。 | v1.0 | tools/capabilities |
| P-1004 | 多言語プラグイン | プラグインは Rust 以外でも実装可能(IPC 規約を満たす任意のプロセス)。 | v1.0 | plugins/ipc |
| P-1005 | プラグイン監督 | spawn・ネゴシエーション・ヘルス・再起動・サーキットブレーカー。 | v1.0 | platform/process-model |
| P-1006 | サイドカー | ローカルエンジン(llama-server 等)を子プロセスとして起動・監視・終了するパターン。 | v1.0 | plugins/broker |
| P-1007 | イベントの2モード | waterfall(横取り可能)と emit(通知)の2モードを持つイベント系。 | v1.0 | core/session-log, core/agent-loop |
| P-1008 | コミュニティプラグイン | 署名カタログ+ハッシュによるサードパーティプラグインの安全な導入。 | v1.0(形式) + 後継(配信基盤) | plugins/manifest-and-profile |

---

- 前: [vision.md](vision.md) / 次: [done.md](done.md)
