# Subsystem間 Interface Boundary / Command・Query・Result Contract — Step 13 Concrete Design

本書は、サブシステム間のインターフェース境界、およびコマンド・クエリ・結果の契約を定める具象設計書です。[対応関係・識別](correspondence-identity.md)（CI）、[永続化・リカバリ](persistence-recovery.md)（PR）、[並行性制御](concurrency-control.md)（CCT）で定めた識別子、データ保存区分、原子性／リカバリ、並行処理に関する契約を前提とし、これらを勝手に変更しません。上位設計との優先順位や矛盾が生じた場合の取り扱いは、[設計文書 README](../README.md#設計文書の優先順位と信頼できる情報源) に従います。なお、本文中の略称 SO / DR / CC は、それぞれ [状態所有権の原則](../architecture/state-ownership.md) / [依存関係ルール](../architecture/dependency-rules.md) / [横断的関心事の設計](../architecture/cross-cutting.md) を指します。

本書に記載されている Rust の疑似コード（pseudo-type / pseudo-trait）はコンパイルを意図したものではありません。型名やフィールド名の分かりやすい同義語への置き換えは許容されますが、型の明確な分離とフィールドの意味論は厳格に維持してください。クレート分割の詳細は [クレート・モジュール分割](crate-module-decomposition.md)、IPC 通信スキーマは [Host↔Client IPC](host-client-ipc.md) にて定めます。

## 1. 対象と非対象

### 1.1 本書が具体化するもの

- **意味論的な境界（semantic boundary）の抽出**: どの境界を明示的なインターフェースとして表現すべきかの導出基準（第3節）。
- **主要インターフェースの契約詳細**: 各インターフェースにおける要求・コマンドの開始責務、最終的な判断権限を持つ担当責任者（authoritative owner）、必要とされる識別子・リビジョン・世代、由来（provenance）・目的・適用範囲、期待される現在の前提条件、権限・同意・利用枠の照合、キャンセルや安全保留との関係、結果の確信度、期限切れ（stale）／拒絶（rejected）／再検証待ち（needs-revalidation）などの判定結果、永続化・コミットとの連携（第4〜9節）。
- **通信・対話区分の明確化**: Command（指示）、Query（照会）、Candidate（提案候補）、Decision（確定判断）、Fact（確定事実）、Observation（観測事実）、Result（処理結果）、Notification（通知）の厳格な使い分け（第2節、第4〜9節）。
- **境界トークン（boundary token）の具体的な引き渡し方法**: 巨大で何でも入った共通コンテキスト構造体を安易に引き回さない規約（第10節）。
- **ドメイン判断による結果と技術的エラーの分離**: 業務上の成否判定とシステム障害を混同せず、巨大な共通ステータス enum を作らない設計（第11節）。
- **長時間処理の要求と完了の分離**: 要求の受付と処理の完了報告を切り離し、永続化された対応関係で安全に追跡する仕組み（第12節）。
- **リポジトリの比較照合コミット（compare-and-commit）インターフェース**: データベースの低レベルなトランザクションをビジネスロジック層へ露出させない設計（第13節）。
- **エラーモデルの配置方針**: ドメイン判定と技術的例外の整理（第11節）。
- **IPC 対応準備**: プロセス間やネットワークを越える可能性があるインターフェースの明確な特定（第15節）。
- **クレート・モジュール分割へのインプット**: クレート間の健全な依存方向（第16節）。
- **検証シナリオ**: インターフェースの整合性を追うウォークスルー（第17節）。

### 1.2 本書が決めないもの

- プロジェクト全体のクレート構成、モジュール分割、プロセスやスレッドの具体的な配置。
- データベース製品の選定、完全な `CREATE TABLE` 文、インデックス設計、マイグレーション手順。
- 具体的な IPC 方式、通信フレームのバイト長仕様、ネットワークプロトコル、ハートビートやリースの有無および具体値。
- キューの長さ、再試行回数、タイムアウト値、スケジューリングアルゴリズム、埋め込みベクトルの検索スコアリング、プロンプト組み立てアルゴリズム。
- 各サブシステムの完全な内部関数一覧（内部ヘルパー関数の網羅は行いません）。
- 具体的な暗号化方式、アーカイブ形式、電子署名方式。

### 1.3 用語

- **コマンド（command）**: 意味論的な状態の変更を担当責任者（owner）へ要求すること。要求を受理・確定するかどうかは担当責任者が判断します。コマンドを呼び出せたこと自体は「変更の成立」を意味しません。
- **クエリ（query）**: 現在の意味状態、利用可否、参照可能な範囲を照会すること。照会に成功したからといって、後続の操作がすべて許可されたわけではありません。
- **候補（candidate）**: 採用するかどうかの判断材料として提示される提案（アクション候補、学習形成候補、フォールバック候補、観測候補など）。候補が存在することと、採用が決定したことは別です。
- **判断（decision）**: 担当責任者が下した確定的な判定（実行許可、スコープ決定、ルーティング採否、全域削除完了など）。有効な判断を別の場面で勝手に再利用できるかどうかは、個別の契約に従います。
- **確定事実（fact）**: システムが実際に把握した外部作用、利用量、試行、状態遷移などの記録。「確認済み成功」「確認済み失敗」「成否不明」といった確信度（certainty）を伴います。
- **観測事実（observation）**: クライアントからの申告、ネットワーク到達性、画面への提示状況などの観測データ。システム全体の決定権威（authority）ではありません。
- **処理結果（result）**: 長時間処理から遅れて届いた成果物。元の試行ID、処理区間、世代番号への紐付けを伴います。結果が届いたからといって、現在の目的へ自動採用されるわけではありません。
- **通知（notification）**: イベント配信などの一方向の伝達。相手が通知を受信したことと、内容を正しく理解・採用したことは別です。
- **境界トークン（boundary token）**: 各サブシステムが期待する前提条件の写し（CI §4.5）。決定権威そのものではなく、現在の状態と比較するための照合材料です。値が不一致の場合は安全のため保留、拒絶、情報不足、再検証へと戻します。トークンが省略された場合は「制約がない」のではなく「前提が不明なため不受理」として扱います。

## 2. Interface 設計原則（採用した原則）

1. **要求は単なる提案、確定は担当責任者（owner）が行う。**
   すべてのコマンドや候補は「これを採用してほしい」という提案に過ぎず、決定権威を持つ判断は状態所有権の原則（SO）に従って担当責任者が行います。インターフェース名に `execute`、`apply`、`update` と付いていても、内部では必ず前提条件の照合と担当責任者による確定処理を経ます。「関数を呼び出せたこと」や「引数の構造体を組み立てられたこと」を、実行権限の根拠にしてはなりません。
2. **前提条件は型として明示し、暗黙の最新状態を勝手に使わない。**
   各インターフェースは、必要な前提条件（`expected_*`）を明示的なフィールドとして受け取ります。リビジョン番号や世代番号を生の数値だけで単独で引き回してはならず、必ず `(identity, revision)` や `(lifecycle, generation)` のペアとして扱います（CI §4.2–4.3）。前提トークンが欠落している場合は、無条件に処理を進めるのではなく「前提不明による不受理」とします。
3. **意味の異なる相互作用を、1つの万能型へ安易にまとめない。**
   システム共通の万能コマンド型、万能イベント型、万能結果エンベロープ、汎用ワークフロー型、汎用リポジトリ、汎用ポリシーエンジン、サービスロケーター、汎用メッセージバスなどを導入してはなりません。将来的に通信用フォーマットを共有できたとしても、ドメイン層における意味論的な型は領域ごとに明確に区別します。
4. **コミットは短く、長時間の作業はトランザクションの外で行う。**
   LLMによる推論、タスクエージェントの自律思考、外部ツールの呼び出し、Computer Use、バックアップ復元、個人データ完全削除の各参加者の消去作業などの長時間処理は、前提条件を確認した上で開始し、データベースのロックやトランザクションを持たずに実行します。そして、結果をコミットする直前の短い不可分（atomic）な処理においてのみ、現在の最新状態と比較照合します（CCT 第5節）。DBトランザクションの内部で非同期処理の await や外部ネットワークI/Oを行ってはなりません。
5. **巨大で何でも入った共通コンテキスト構造体を渡さない。**
   各境界で必要とされる前提条件だけを、厳格に型付けして受け渡します（第10節のフィールドマトリクス）。共通化してよいのは不透明性（opaque）、単調増加性、方向性、比較の形式といった基本性質だけであり、具体的な型・ライフサイクル・カウンターをシステム全体で無理に共有してはなりません（CI §2.2）。
6. **結果はドメインごとに明確に区別し、技術的エラーと業務判断を分離する。**
   すべての成否を単純な `Result<T, E>` にひとまとめにせず、`Result<DomainAcceptance, TechnicalError>` のように型を分けて返します（第11節）。期限切れ（stale）、権限拒絶（denied）、安全保留（held）、後続処理による置換（superseded）、権能不足（insufficient）、外部成否不明（unknown-external）、履歴記録のみ（historical-only）などは、すべて正常系（`Ok` 側）のドメイン判定結果（outcome）であり、DB接続失敗やネットワーク通信障害などの技術的例外（`Err` 側）と混同してはなりません。システム共通の巨大なステータスコード enum を作ってはなりません。
7. **長時間処理は永続化された対応関係によって安全に元の文脈へ戻れる。**
   要求オブジェクトをプロセスメモリの中だけで保持してはなりません。チケットID、試行ID、操作ID、ステージング領域などの永続化された対応情報から、元の識別子、リビジョン、世代、試行番号、由来をいつでも確実に辿れなければなりません（第12節）。
8. **秘密情報を通常の戻り値として不用意に返さない。**
   認証情報管理サブシステムは、各処理に必要な認証機能を提供することはあっても、呼び出し元へ秘密値そのものの所有権を渡してはなりません（DR-05）。秘密でない参照情報と、秘密値そのものは、まったく別の意味を持つ型として厳格に区別します。
9. **トレイトの導入は、境界の隔離・部品の差し替え・テスト容易性に明確な理由がある場合のみに限定する。**
   サブシステム内部の純粋な計算処理まで無闇にトレイト化してはなりません（第14節）。

## 3. Boundary discovery — どの boundary に明示的 interface が必要か

### 3.1 導出方法

サブシステムの責務（個体調整・作業・学習・権限・接続・Character・保全消去）、識別子の対応関係、永続化テーブル区分、横断関心事、並行性制御の同期区分を整理し、以下の基準に基づいて明示的なインターフェースを導出しました。

**明示的なインターフェースの定義が必須となる条件（以下のいずれかに該当する場合）：**

- (a) **担当責任者（semantic owner）を跨ぐ受け渡しがある場合**: 要求を開始する責務と、最終的に判断を下す責務が異なっている。
- (b) **現在の前提条件の照合が必要な場合**: 権限、同意、利用枠、在席帰属、消去保留、復元保留、方針指示のいずれかが要求の受入可否を左右する。
- (c) **ライフサイクル・リビジョン・世代を跨ぐ対応が必要な場合**: 遅れて届いた結果の帰属判定、指示変更の前後の区別、バックアップ復元の前後の区別、消去対象期間の区別など。
- (d) **永続化コミットやアトミックな比較照合が必要な場合**: 永続化の最小単位や並行性制御の同期区分に対応する。
- (e) **信頼境界（trust boundary）を跨ぐ場合**: クライアント、外部プロバイダ、MCPサーバー、プラグイン、外部ファイル、認証秘密情報とのやり取り。

**内部ヘルパーに留め、公開インターフェースへ昇格させてはならないもの（例）：**

- プロンプトの組み立て、文脈の圧縮・選択アルゴリズム、埋め込みベクトルのスコアリング、差分マージアルゴリズム、タスクのスケジューリングアルゴリズム、費用の概算計算式、集計結果の表示フォーマット整形、描画や音声のローカルステージング、候補検知の内部スコアリング、検索クエリの組み立てなど。これらは担当者が判断を下すための内部手段であり、独立した所有権境界ではありません。
- 単なる表示用の集計処理、由来説明文の文章生成、次回実行日時の計算処理など。これらは派生データを生成する手段に過ぎず、マスターデータの受け渡しではありません。

なお、「サブシステム」は必ずしも個別のクレート、プロセス、アクターと1対1に対応するわけではありません。同一プロセス内や同一クレート内であっても、所有権境界を守るために明示的なインターフェースを定義します（例：タスクの一時エージェントが勝手に自律学習へデータを昇格させることを防ぐ境界や、権限評価と実アクション実行の分離など）。逆に、同一の担当責任者の内部における処理ステップの分割は、インターフェースとしては公開しません。

### 3.2 Interface inventory（明示的 interface の一覧）

※ 以下の ID は本書内での参照用記号であり、製品要件の ID ではありません。「種類」は主な相互作用の形態、「開始」は要求を開始する責務、「owner」は決定権威を持つ担当責任者を示します。

| ID | 境界・目的 | 種類 | 開始責務 | 担当責任者（owner） | 対応する設計契約 |
|---|---|---|---|---|---|
| H-A | Task 化・委任・方針指示（steering）・中断・結果統合 | コマンド ＋ 確定事実 | 個体調整 | 作業 | H-3、W-1〜W-3、AE §4・§7、CC-01/03/04/07 |
| H-B | Experience（経験）提出 → 形成判断 | 候補 ＋ 確定判断 | 個体調整・作業 | 認識・学習 | H-1、L-1〜L-2、CA |
| H-C | 会話による学習内容の訂正 | コマンド（訂正要求） ＋ 確定判断 | 個体調整 | 認識・学習 | H-2、L-3〜L-6 |
| H-D | Learning 参照・由来説明の照会 | クエリ ＋ 確定事実 | 個体調整・作業・共有観測（範囲限定） | 認識・学習 | H-4、L-8、CC-02 |
| H-E | 学習スコープの変更（Companion専用／全体共有） | 候補 ＋ 確定判断 | 認識・学習（意味判断） | 認識・学習（意味）＋ 権限・制約および各利用箇所（強制適用） | H-5、L-7、CC-02 |
| H-F | Observer 向け限定ルーティング文脈の供給 | 確定事実（派生データ） | 個体調整との協働（仲介） | 各元の担当責任者（会話履歴・個体文脈＝個体調整、記憶・学習＝認識・学習、タスク文脈＝作業） | H-6、X-5、OB-4、CC-02 |
| H-G | 未伝達メッセージの登録・報告状況更新 | 確定事実 ＋ 観測事実 | 個体調整（管理）＋ 入出力・提示（提示事実） | 個体調整（必要性の判断・状況管理） | H-7、X-9、SO 4.4 |
| H-H | スケジュール設定・到来・各回実行 | コマンド ＋ 確定事実 | オーナー（管理経路）→ 作業 | 作業 | H-8、W-4 |
| K-A | オーナー意図 → 制御変更（ルール・同意・利用枠等） | コマンド ＋ 確定判断 | 入出力・提示 ＋ 個体調整・作業（意図の伝達） | 権限・制約 | K-1、C-1、CC-01 |
| K-B | 現在の利用可否の照合（live authorization check） | クエリ ＋ 確定判断 | 利用・実行箇所（推論・実行・拡張・各参照保存箇所） | 権限・制約 | K-2、C-7、AE §5、CC-01 |
| K-C | 秘密情報の安全利用（認証用途への供給） | コマンド（利用要求） ＋ 確定事実 | 推論・実行・拡張・接続・存在 | 認証秘密情報 | K-4、S-2〜S-4、CC-02 |
| K-D | モデルプロバイダ割り当ての解決 | クエリ ＋ 候補 | 推論（解決処理） | 権限・制約（同意の意味論） | K-5 前半、I-1、SO 4.18 |
| K-E | 推論の実行成立・プロバイダ送信 | コマンド ＋ 確定事実／結果 | 利用元（個体調整・作業・認識・学習・共有観測・入出力・提示） | 推論（経路・能力照合）＋ 権限・制約（可否判断）＋ 各送信箇所（適用） | K-5、I-4、RF §3、CC-02 |
| K-F | フォールバック先の選択 | 候補 ＋ 確定判断 | 推論（候補提示） | 権限・制約（承認済み順序・現在条件の照合） | K-6、I-5、CC-02 |
| K-G | 利用枠の事前予約・確定・解放 | コマンド ＋ 確定事実 | 各利用元（推論・作業・実行・拡張・保全・消去） | 権限・制約（可否判定）＋ 各利用元（元記録の保持） | K-7、C-4、CC-06、CCT 第9節 |
| K-H | アクション候補 → 認可 → 実作用 → 結果確定 | 候補 ＋ 確定判断 ＋ コマンド ＋ 確定事実 | 利用元（候補提示）→ 権限・制約（認可）→ 実行・拡張（実作用・確信度） | 権限・制約（認可）・実行・拡張（作用・確信度）・作業（タスク達成判断）で分離 | K-8、E-1〜E-2、AE §3・§5・§6、CC-01/07 |
| K-I | 外部拡張の受け入れ（MCP／Plugin／MCP Apps／サンドボックス例外） | コマンド ＋ 確定判断 ＋ 確定事実 | 機能利用元 | 実行・拡張（受け入れ・制限）＋ 権限・制約（例外の意味論） | K-9、E-4〜E-6、C-5 |
| K-J | クライアント依存アクションの限定・端末移動時の協調 | コマンド ＋ 確定事実 | 作業・個体調整（要求） | 実行・拡張（実作用）＋ 接続・存在（在席帰属）＋ 権限・制約（デバイス・許可） | K-10、E-3、X-6、CC-04 |
| K-K | 失効・停止・安全保留の伝播、遅延到着・成否不明の処理 | 通知 ＋ 確定事実 | 権限・制約・個体調整・保全・消去（状態成立） | 各開始箇所（新規開始禁止の適用）＋ 各担当責任者（事実の帰属判定） | K-11、C-8、AE §7–§8、CC-03/04 |
| X-A | 在席（presence）の成立・端末間移動・復帰・復旧 | コマンド ＋ 確定判断 ＋ 確定事実 | 個体調整（移動意図の表明） | 接続・存在（帰属の確定） | X-1、CN-2〜CN-4、CC-04 |
| X-B | テキスト／音声ラウンドの帰属・区切り・提示完了 | 候補 ＋ 確定事実 ＋ 観測事実 | 入出力・提示（受付・提示事実） | 個体調整（対話の意味論）＋ 入出力・提示（ラウンドの実態）＋ 接続・存在（在席帰属） | X-2、IO-1〜IO-2・IO-6、CC-07 |
| X-C | アバター身体（Body）表示の帰属 | クエリ（帰属参照） ＋ 確定事実 | 入出力・提示 | 接続・存在（帰属）・個体調整（活動状態）・認識・学習（内的状態の意味論）・Character（資材データ） | X-3、IO-3 |
| X-D | 観測資格（eligibility）の連動 | 通知 ＋ 確定判断 | 接続・存在・入出力・提示（状態変化の通知） | 共有観測（対象・タイミングの判定） | X-4、OB-1、CC-04 |
| X-E | 観測結果のルーティング（三段階の分離） | 候補 ＋ 確定判断 ＋ 通知 ＋ 確定事実 | 共有観測（候補提示・ルーティング） | 共有観測（ルーティング採否）・個体調整（理解・発話・アクション）で分離 | X-5、OB-2〜OB-6、CC-02/03 |
| X-F | 端末移動・切断・再接続時のサブシステム協調 | 通知 ＋ 確定事実 | 接続・存在・入出力・提示・実行・拡張 | 接続・存在（切り替え）・入出力・提示（区切り）・実行・拡張（作用不明の記録）・作業（ホスト継続）・個体調整（未伝達管理） | X-6、CN-6、CC-03/04 |
| X-G | ホスト再起動時の整合性協調 | 確定事実（保全されたマスターデータからの再構成） | 保全・消去（永続化データ） | 接続・存在・入出力・提示・共有観測・作業・個体調整（各用途での復元照合） | X-7、CC-04 |
| X-H | 未伝達メッセージの次回接続端末への要約報告 | 確定事実 ＋ ドメイン判定結果（要約の提示完了） | 個体調整（未伝達・要約管理） | 入出力・提示（クライアント向け提示） | X-8、CC-04 |
| C-A | キャラクター静的リビジョンの取得 | クエリ ＋ 確定事実 | 個体調整（適用判断のため） | Character（静的資材の提供） | CH-1〜CH-4、CD（供給） |
| C-B | 既存パートナーへのキャラクター改訂の適用 | 候補 ＋ 確定判断 ＋ 確定事実 | 個体調整（オーナー選択を伴う提案） | 個体調整（適用関係の確定）・Character（内容提供）で分離 | CH-7、H-5 の適用側、CC-02 |
| C-C | スキルのインポート・パッケージ正当性検証 | コマンド ＋ 確定事実 | オーナー（管理経路） | Character（受け入れ・検証）・認識・学習（内部スキルとしての意味論）で分離 | CH-5、SO 4.7 |
| C-D | キャラクターのエクスポート | コマンド ＋ 確定事実 | オーナー（管理経路） | Character（静的範囲の選定） | CH-6、CC-02 |
| D-A | 個人データ完全削除の要求受付・範囲確定 | コマンド ＋ ドメイン判定結果（削除範囲の確定・受付） | オーナー（管理経路）→ 保全・消去 | 保全・消去 | DP-1 (PE-1)、TD |
| D-B | 各ドメインの局所消去・残存検証要求 | コマンド ＋ 確定事実（残存なし／未確認範囲の報告） | 保全・消去（協調・照合） | 全参加ドメイン（永続化担当者） | DP-1 (PE-1)、TD |
| D-C | システム全域での削除完了確定・保留解除 | 確定事実（全域完了の確定事実） | 保全・消去 | 全サブシステム | DP-1 (PE-1)、TD |
| D-D | バックアップ作成・復元ステージング・マスター切り替え・復元後保留・一括有効化 | コマンド ＋ 候補 ＋ 確定判断 ＋ 確定事実 | オーナー（管理経路）→ 保全・消去 | 保全・消去（調整）＋ 各担当責任者（参加・保留適用） | DP-2〜DP-4 (PE-2〜PE-4)、BR |
| D-E | 通常のデータ保持・クリーンアップ・完全初期化 | コマンド ＋ 確定判断 | オーナー（管理経路） | 保全・消去（調整）＋ 各担当責任者（局所処理） | DP (PE-4〜PE-5)、CC-05 |
| R-* | リポジトリの比較照合コミット群（第13節） | コマンド（比較照合付き） ＋ 確定事実 | 各ビジネスロジック担当 | 各データ保存区分の担当責任者（意味変更は業務担当、永続化・整合性はリポジトリ） | PR §7・§11、CCT 第5節、CI §6 |

インターフェースを呼び出せること自体を、処理の実行権威にしてはなりません。たとえば、`H-A` のタスク化要求が届いたことはタスクの正式受理を意味せず、`K-H` のアクション候補が組み立てられたことは外部作用の実行許可を意味せず、`K-C` の認証情報参照を持っていることは秘密値の自由な利用を意味せず、`X-B` のクライアントメッセージを受信したことは現在も在席が有効であることを意味せず、`H-A` のエージェント処理結果が届いたことは現在のタスクへの自動採用を意味しません。必要な前提条件の照合は、必ず各担当責任者の境界において厳格に実行します（第4〜9節の `expected_*` フィールドおよび第11節の判定結果型）。

## 4. Task / Learning / Companion クラスタの interface semantics（H-A〜H-H）

### H-A Task 化・委任・steering・Cancel・結果統合（個体調整 → 作業）

タスク管理と自律学習を、単一の汎用ワークフローインターフェースへ安易に統合してはなりません。タスクは状態追跡を伴う明確な作業単位であり、自律学習は認識や解釈に関する意味論的な判断であって、両者は担当責任者、リビジョン管理、ライフサイクルのすべてが明確に異なります。

```rust
// --- 識別子・リビジョンの前提型（CI §5.3 の再利用。同義の命名変更は許容） ---
struct TaskId(/* 不透明な識別子 */);
struct TaskRevision(u64);
struct TaskRef { task: TaskId, revision: TaskRevision }

// 採用された目的。識別子は採用位置が持ち、本文のマスターデータは task_revision のスナップショット。
// 識別子は作業担当が採用時に確定し、呼び出し側は本文を提案するのみ（呼び出し側では発行しない）。
struct TaskPurposeRef {
    task: TaskId,
    adopted_revision: TaskRevision,  // この目的を採用したリビジョン
}
struct TaskPurpose { text: String }  // 採用目的本文。デバッグログでは伏字化（redact）する

// 方針指示（steering）・遅延結果の比較材料（境界トークン。決定権威そのものではありません）。
// expected は依拠した現在リビジョン、purpose は依拠した現在目的。
// premise.purpose は expected.revision の目的と一致すること（不一致は前提不一致として不受理）。
// orchestrate が premise 構築時にこの対応を照合し、repository のリビジョン比較更新（atomic compare）が現在性を包含します。
struct SteeringPremiseRef {
    expected: TaskRef,
    purpose: TaskPurposeRef,
}

// タスク側の相手識別子の前提型。他ドメインの固有型を直接インポートしない。
struct AssigneeRef { companion: RawId }  // 担当パートナー（CompanionId とは直接変換しない）

// 採用されたコンテキスト項目（entry）。各項目は採用識別子・由来・取得日時を持ちます。
// AU2/AU4 のリポジトリスライスが記録した種類は採用目的のみです。採用指示項目は H-A steering 配線スライスが
// 生成元（producer）とともに追加し、材料や途中理解はそれぞれの利用先が分岐するスライスが同じ形で追加します。
// 本文のマスターデータは元のレコード（採用目的: task_revision スナップショット、採用指示: 会話履歴）にあり、項目側では本文を重複複製しません。
struct TaskContextEntryId(/* 不透明な識別子 */);
enum TaskContextItem {
    AdoptedPurpose(TaskPurposeRef),
    // 採用指示。採用識別子は項目自身の TaskContextEntryId であり、由来レコード（会話: 会話履歴レコード）は
    // origin.source が参照します。本文は複製しません。項目は採用リビジョンで 1 度だけ書き込み、以降の前進（forward）では
    // 再記録しません。現在有効な指示は、現在リビジョンまでの AdoptedInstruction 項目全体となります
    // （破棄・退役（retire）の生成元が存在するまで。破棄は項目を削除・再記録するのではなく、識別子の置き換え（supersede）として表現し、
    // 生成元を持つスライスが書き込み前提・移行・現在有効な識別子の比較読み出し規則を同一の設計変更で
    // 追加します。それまではすべての採用指示項目が現在有効として扱われます）。
    AdoptedInstruction,
}
struct TaskContextOrigin {
    kind: TaskContextOriginKind,   // 会話・自発的行動・スケジュール各回等の由来
    source: RawId,                 // 由来レコードの識別子（会話: 会話履歴レコード、自発: 活動記録、
                                   // スケジュール: 実行回。本文は重複複製しない）
}
enum TaskContextOriginKind { OwnerConversation, Spontaneous, ScheduleOccurrence }
struct TaskContextEntry {
    entry: TaskContextEntryId,
    reference: TaskRef,            // 採用された (task, revision)
    item: TaskContextItem,
    origin: TaskContextOrigin,
    acquired_at: WallClockWithTz,  // 取得日時
}

// ワークスペース利用条件（呼び出し側の値の前提。関連付けの識別子そのものではない）。
// 外部フォルダはパス参照のみを保持し、ene は所有しない。関連付けの確定は作業担当が行う。
struct WorkspaceFolderRef { path: String }
struct WorkspaceNeedRef {
    folder: WorkspaceFolderRef,
    save_target: Option<WorkspaceFolderRef>,  // 未定なら None（最終保存前に確認する）
}

struct DelegationId(/* 不透明な識別子 */);
struct TaskAgentEphemeralId(/* 不透明な識別子 */);
struct WorkspaceAssocId(/* 不透明な識別子 */);

// 委任スコープ（DelegationScope / DelegatedWorkspace）の定義と凍結規則は CI §5.3 にあります。
// 境界情報であり、権限そのものではありません。パス解決・トラバーサル遮断・現在の権限照合は
// 実行時の実行・拡張担当が行い、本構造体を生きた許可や解決済み経路として再利用してはなりません。

// タスク化・委任の要求。個体調整が要求を開始し、作業担当が受理・反映・達成を確定する。
struct ProposeTaskCommand {
    requester: CompanionId,          // 委任元パートナー（個体調整の同一性）
    purpose: TaskPurpose,            // 採用を提案する目的本文（識別子は作業担当が確定）
    origin: TaskContextOrigin,       // 会話・自発的行動・スケジュール各回等の由来
    workspace_need: Option<WorkspaceNeedRef>, // ワークスペース利用条件（関連付けは作業担当が確定）
    client_binding: Option<ClientBindingPremise>, // クライアント依存条件（受入時の現在条件。永続化しない）
}

// 受入側前提（呼び出し側が要求型から写すタスク側の値。識別子は含めない）。
// TaskId・TaskContextEntryId・WorkspaceAssocId は作業担当（orchestrate）が発行し、
// TaskCreationPremise を構成してリポジトリへ渡します。方針指示（AU4）の新リビジョンが
// 記録する採用目的・採用指示項目の TaskContextEntryId も同じく作業担当が発行し、
// TaskCommitPremise で渡します（リポジトリは渡されたコンテキスト項目識別子を採番し直しません）。
struct TaskProposalPremise {
    requester: AssigneeRef,          // ProposeTaskCommand.requester の写し
    purpose: TaskPurpose,
    origin: TaskContextOrigin,
    workspace_need: Option<WorkspaceNeedRef>,
    client_binding: Option<ClientBindingPremise>, // 受入時の現在条件。永続化しない
}
struct ProposeSteeringCommand {
    premise: SteeringPremiseRef,     // 依拠したリビジョン・目的（境界トークン）
    new_purpose: Option<TaskPurpose>, // None = 目的変更なし（直前の目的を引き継ぐ）
    instruction_source: RawId,       // 採用を提案する追加指示の発言レコードの参照（採用判断・採用識別子の発行は作業担当）
    // unadopted フィールド（未反映・待機の対応）: W-3 の反映可否・反映不能の理由と選択肢（requirements.md §Task）を
    // 生成元に持つ最初のスライスが、コマンドフィールドか結果の列挙子（outcome variant）かを読み出し規則とともに
    // 同一の設計変更で追加します。H-A steering 配線スライスはこのフィールドを含めません。
}

// 作業担当が採用した追加指示の識別子は、新リビジョンのコンテキスト項目（指示項目は H-A steering 配線スライス
// で生成元とともに追加）が表します。会話発言レコード（会話履歴のマスターデータ）を採用済み指示と同一視してはなりません。
// 呼び出し側が渡す instruction_source は由来レコードの参照であり、採用識別子や項目識別子とは明確に区別されます。
// 採用指示の採用識別子は、新リビジョンの採用指示項目（entry）の TaskContextEntryId です。項目は採用リビジョンで
// 1 度だけ書き込み、以降の前進（forward）で再作成しません。したがって採用識別子はリビジョンを跨いで安定し、instruction_source
// （会話履歴レコードの RawId）とは別個の概念です。現在有効な採用指示は、現在リビジョンまでの AdoptedInstruction 項目
// 全体として読み出されます（破棄・退役の生成元が存在するまで）。

// 受入側前提（方針指示。呼び出し側の要求型から写すタスク側の値。識別子は含めない）。
// 作業担当（orchestrate）が採用目的・採用指示項目の識別子を発行し、TaskCommitPremise を構成します。
struct SteeringProposalPremise {
    premise: SteeringPremiseRef,       // 期待リビジョン・依拠目的（境界トークン）
    new_purpose: Option<TaskPurpose>,  // 目的変更の提案本文。採用識別子・由来・取得日時は作業担当が確定
    instruction_source: RawId,         // 採用を提案する追加指示の発言レコードの参照（採用識別子そのものではない）
}

struct CancelTaskCommand {
    task: TaskId,                    // 中断対象のタスク。リビジョンは問わない（最新状態への中断記録のため）
    reason: CancelReasonRef,
}

// 作業担当による確定結果。会話上での受付と、タスク本体への反映を明確に区別する。
enum TaskProposalOutcome {
    AcceptedAsTask(TaskRef),         // 新規タスクとして正式に受理
    AcceptedAsSteering(TaskRef),     // 新しいリビジョンとして方針変更を受理（過去リビジョンも保持）
    CancelAccepted,                  // 中断要求を現在のタスクに記録（外部処理の停止完了とは別）
    StalePremise { current: TaskRef }, // 期待リビジョン・依拠目的の不一致（再評価へ差し戻し）
    TaskTerminal { task: TaskId, progress: TaskProgress }, // 方針指示: 対象 Task が terminal（Completed / Failed）のため
                                   // 前進できない（書き込みなし。リポジトリ結果の写し）。終端は吸収的であり、revision stale と区別する
    MissingTask { task: TaskId },       // 方針指示: 前提のタスクに永続状態が存在しない（書き込みなし。リポジトリ結果の写し）
    RevisionExhausted { task: TaskId }, // 方針指示: 次の相異なるリビジョンを永続的に確定できない（書き込みなし。リポジトリ結果の写し）
    HeldByGlobalHold(HoldConditionRef), // データ消去中・復元保留・安全停止などによる新規禁止（hold スライスで導入。ペイロードは受入側オーナーが定義する前提型に写す）
    NeedsRevalidation(NeedsRevalidationRef), // 権限・在席帰属・利用枠などの再照合が必要
    InsufficientContext(InsufficientRef),    // 目的やワークスペース条件が不足している
}

// 一時的な Task Agent への委任作成。作業担当が確定し、Agent は一時的な従属主体に留まる。
// DelegationId・TaskAgentEphemeralId は作業担当（orchestrate）が発行し、リポジトリは
// 採番し直しません（TaskCreationPremise と同じ規則）。
struct CreateDelegationCommand {
    task: TaskRef,                   // 期待するタスクリビジョン（boundary token）
    scope_copy: DelegationScope,     // 委任時のワークスペース境界の写し（独立した特権を与えない。タスク範囲は task が示す）
    // consumer_assignment（推論・費用の消費主体割り当て）は、保存された identity としては持ちません。
    // 割り当てを委任作成時に凍結せず（凍結した解決済み経路は権限ではありません）、Task Agent の推論受付
    // （K-E）時に委任元が依拠する現在の Capability 同意から live に解決します。durable な帰属は推論試行
    // （InferenceAttempt）の (consumer, purpose, delegation, 依拠 TaskRef) 対応が担い、費用は委任元と
    // 同一の利用枠へ合算します。将来 producer（per-companion override・cap 等）が独立した assignment
    // identity を必要とする場合は、その producer が command・premise・マイグレーション・読み出し規則を
    // 同じ設計変更で追加します。
}

enum DelegationOutcome {
    Delegated(DelegationRef),
    StaleTaskRevision { current: TaskRef },
    TaskTerminal { task: TaskId, progress: TaskProgress }, // 対象 Task が terminal（Completed / Failed）のため新規委任を
                                   // 作成できない（書き込みなし。§11.3 に従い Ok 側のドメイン判定結果）。再評価しても前進しないため
                                   // revision stale と区別する
    MissingTask { task: TaskId },    // 前提のタスクに永続状態が存在しない（書き込みなし。§11.3 に従い Ok 側のドメイン判定結果）
    // HeldByGlobalHold は hold スライスが、NeedsRevalidation・InsufficientContext は権限・
    // 在席・利用枠・情報不足の再照合 producer を持つスライスが、対応する前提型と対で追加します。
    // それまでは具象 enum に代役バリアントや仮の前提条件を置いてはなりません。
}

// 長時間処理の分離：委任の作成要求（request）と Agent からの結果到着（completion）は別のインターフェース。
// 結果の到着・採用は作業担当（Task owner）が確定し、provider 出力・採用・外部作用成功を混同しません。
// Task Agent の自己申告は外部作用の確信度語彙として受け付けず、結果の identity / 本文 / 相関だけを運びます。

// 結果本文の durable な identity。Task Agent execution（1 delegation）が final result を提出した
// 明示的な finalization 境界で作業担当（orchestrate）が発行し、リポジトリは採番し直しません。
// 1 delegation につき final result は最大 1 つであり、同じ identity の retry は冪等です
// （同じ identity の再 arrival でも本文は 1 行を超えて保存されず、採用判定は同じ結果行に対して再評価されます）。
struct TaskResultId(/* 不透明な識別子 */);

// 結果の到着（durable な record premises）。final Task result を提出する明示的な finalization 境界でのみ呼ばれ、
// 依拠タスクリビジョンは delegation 行（委任の対応関係）が正本であり arrival は再掲しません。
// provider 出力を受け取った orchestrate が adoption より先にこの記録を 1 回だけ確定し、本文を失いません
// （durable-before-visible）。この step は現在性・確定度・完了・terminal のいずれも判定しません。
// 1 回の inference turn の provider 出力（TaskAgentTurnOutcome::Produced）は final result ではなく、
// ここへ入れて execution を seal してはなりません（intermediate output の seal 禁止）。
struct TaskAgentResultArrival {
    delegation: DelegationId,        // 委任の対応関係（依拠タスクリビジョンは行から解決）
    result: TaskResultId,            // 結果本文の identity。orchestrate が finalization 境界で発行
    body: TaskAgentOutput,           // final answer の provider 出力。Task 側で 1 度だけ durable 保存（本文の複製を作らない）
}

// 採用判定へ渡す主張。attempt_refs は caller（orchestrator / Task Agent 経路）の自己申告であり、
// それ自体は完了の証拠になりません。store は result 行の delegation（1 delegated Task Agent execution の
// lifetime。0..N inference turn / 0..N Action attempt / 0..1 final result）から、その execution に属する
// Action attempt の authoritative set（seal 時点で固定された完全な durable 集合）を列挙し、
// この集合との完全一致（欠如・追加・重複のいずれも不可）を要求します。
struct TaskResultAdoptionClaim {
    result: TaskResultId,
    attempt_refs: Vec<RawId>,        // 主張。durable な execution lifetime から列挙した authoritative set との完全一致でのみ採用材料になる
}

// 採用判定の結果。すべて Ok 側のドメイン判定であり、技術的エラー（主張と durable 相関の不一致などの破損）と混同しません。
enum TaskResultAcceptance {
    AdoptedAsCompletion(TaskRef),    // 結果は採用と同じ不分区間で 1 回だけ確定済み（到着 record 済み）で、現在リビジョン・目的 identity と
                                     // 一致し、delegation（execution lifetime）から列挙した authoritative set が claim と完全一致し、
                                     // すべて ConfirmedSuccess で、同じ TaskId の全 revision / 全 delegation に Unknown が無い
                                     // （Task-wide completion barrier）ため、同じ不分区間で Task を完了
    RecordedToOriginalOnly,          // 現在リビジョンが前進済み / Task が終端済み / 後続 cancel marker あり（producer は後続スライス）。
                                     // 元の依拠リビジョンへの durable record に留め、現在 Task を変更しない
    WithheldByEffectFacts {          // claim が authoritative set と完全一致した上で、現在 Task を完了しない。
        attempts: Vec<RawId>,        // blockers = 依拠試行の ConfirmedSuccess 以外（Unknown / ConfirmedFailure）∪ 同じ TaskId に属する
    },                               // 全 revision / 全 delegation の Unknown（Task-wide completion barrier。集合として重複を 1 回に畳む）。
                                     // Task-wide barrier で見つけた試行は result-local 相関（task_result_attempt）へ追加しない。
                                     // Unknown は確定するまで保持し、Agent の自己申告で昇格させない
    MissingResult {                  // 採用対象の result 行が durable に存在しない（record 前の adoption 等。識別子不在は正常系で書き込みなし）
        result: TaskResultId,
    },
    MissingDelegation {              // 委任に対応する durable 状態が無い（書き込みなし）
        delegation: DelegationId,
    },
    MissingTask {                    // 結果が解決した Task の durable 状態が無い（書き込みなし）
        task: TaskId,
    },
}

// `TaskAgentResultArrival` の `TaskResultBodyRef` / `AgentResultCertainty` と `TaskResultAcceptance` の
// `HeldForPermissionReview` / `DiscardedAsStaleWithRecord` は、producer（結果単位の権限再照合 workflow /
// stale の別語彙）が存在しない、または `RecordedToOriginalOnly` と同義のため削除しました。
// provider 出力そのものを完了・採用とみなさず、結果の到着 record（本文の durable 化）と
// 現在 Task への採用・完了を分離します。
// Task 完了に利用する Action attempt 集合は、arrival caller が任意に省略・追加できる値ではありません。
// delegation（1 delegated Task Agent execution。0..N inference turn / 0..N Action attempt / 0..1 final result）を
// execution lifetime とし、その寿命に属する Action attempt から完全な集合を解決して claim と比較します
// （存在しない attempt を主張に足しても存在する attempt を主張から落としても技術的エラーとして
// fail closed し、Withheld / stale へ丸めません）。
// Task 完了の安全境界はこの result-local 集合だけでは足りません。Completed への CAS と同じ不分区間で、
// 同じ TaskId に属する全 revision / 全 delegation の Action attempt に Unknown が 1 件も無いこと
// （Task-wide completion barrier）を別途要求し、barrier で見つけた attempt は結果の依拠相関として
// task_result_attempt へ追加しません（barrier は完了の可否だけを判定します）。
// final result の到着（AU15a）は delegation を seal し、以後その execution からの新しい inference claim（AU14）と
// Action 開始（AU5）を同じ不分区間で拒否します。seal は Task の terminal lifecycle とは別概念であり、
// Task が InProgress のままでも成立します。

// Task のライフサイクル / 進捗（closed world）。Task revision（目的・指示の steering）とは別軸であり、
// revision の前進や世代番号で lifecycle を代替してはなりません。terminal 状態は吸収的で、
// 結果採用・委任作成・試行開始・後続の cancel marker のいずれも terminal から非 terminal へ戻しません。
// terminal は単なる revision stale ではなく「再評価しても開始できない」ため、本スライスの admission gate は
// 専用の domain outcome（TaskTerminal）で拒否し、Stale* へ丸めません。gate は次の比較と同じ不分区間に置きます:
// AU3 create_delegation、AU4 forward_steering、AU14 Task Agent 推論試行 claim、AU5 Action 開始。
enum TaskProgress {
    Started,      // 開始: Task が正式に受理され、terminal でない限り委任可能
    InProgress,   // 進行中: 少なくとも 1 つの委任が durable に作成され、terminal 判定待ち
    Completed,    // 完了: 作業担当が採用 result と依拠 Action facts を確認して確定
    Failed,       // 失敗: 作業担当が確定した terminal failure。本スライスに producer はなく、
                  // provider failure・Action Unknown・Withheld な結果を写さない（対で producer を持つスライスが追加）
}

// Execution seal（本スライス）: final result の到着（AU15a）が delegated execution（1 delegation）を seal する。
// seal は「その execution が今後新しい work を開始しない」境界であり、Task の terminal lifecycle とは別である。
// seal 済み execution は新しい inference claim（AU14）と新しい Action 開始（AU5）を拒否し、
// 既に durable start した work を「無かったこと」にしない。authoritative set（Action attempt 集合）は
// seal 時点の membership で固定され、以後 membership は増減せず、certainty だけが
// ene-action owner の新しい客観的証拠により更新され得る（set membership と certainty update を混同しない）。
// 1 delegation につき final task_result は最大 1 つ（durable invariant: UNIQUE(task_result.delegation_id) 相当）であり、
// 同じ TaskResultId の retry は冪等、同じ delegation の異なる TaskResultId は 2 つ目の final result として受理しない。
// seal は delegation ごとの task_result 行の存在そのものであり、seal 専用の列・行を別に持たない。
```

- **開始責務と判断権限**: 個体調整担当がタスク化・方針指示（steering）・中断（Cancel）の意図を開始し、作業担当がその受理・反映・達成を確定します。`TaskProposalOutcome` や `TaskRepository` trait などの結果型は作業担当が所有し、呼び出し側（dialogue）は自身の要求型（`ProposeTaskCommand` や `ProposeSteeringCommand` 等）を作業側の値の前提（`TaskProposalPremise` や `SteeringProposalPremise`）に変換して渡します。`TaskId`・`TaskContextEntryId`・`WorkspaceAssocId` などの識別子は作業担当（`orchestrate`）が発行して `TaskCreationPremise` を構成します。方針指示の新リビジョンが記録するコンテキスト項目（採用目的・採用指示等）の `TaskContextEntryId` も作業担当が発行し、呼び出し側は依拠した現在リビジョン（`expected`）のみを渡します。タスク作成時は要求元を担当者 `AssigneeRef` として写します。権限の判断は権限・制約担当が行い、外部アクションの成功判定は実行・拡張担当が行います。
- **方針指示（steering）の結果マッピング**: 作業担当（orchestrate）は、まず `load_task` が返した現在リビジョンに対して `expected` と依拠目的を照合します（`expected != current` の場合は `StalePremise { current }`。`expected == current` であっても `premise.purpose` が現在の採用目的識別子と一致しなければ同様に `StalePremise { current }` とし、単なる目的本文の一致では照合しません）。`forward_steering` の呼び出し後は、`CommittedAs` を `AcceptedAsSteering` へ、`StaleExpected { current }` を `StalePremise { current }` へ変換し、`MissingTask`・`RevisionExhausted`・`TaskTerminal` は同じ意味・ペイロードのまま `Ok` 側のドメイン結果として返します。リポジトリのリビジョン比較照合（atomic compare）が、事前照合後の競合レースを含む現在性を包含します。`HeldByGlobalHold` は hold スライスが保留照合前提と対で追加し、このマッピングに加わります。
- **入力として必要な情報**: 採用を提案する目的本文と発生由来（作成時）、`SteeringPremiseRef`（依拠リビジョン・目的。方針指示時）、委任スコープの写し、ワークスペース利用条件、クライアント依存条件。なお、軽微な処理をタスク化せずに直接行うかどうかの判断は本体が行えますが、その閾値や分類アルゴリズムを固定化してはなりません（H-9）。
- **由来・目的・スコープの保持**: 目的本文、採用位置の識別子、発生由来、委任範囲、ワークスペースの境界情報を決して欠落させてはなりません。単なる会話上の発言記録と、タスクに実際に反映された指示内容、および未反映・保留となった内容を明確に区別します。
- **クライアント依存条件**: `client_binding` は受付時の現在条件として照合し、タスクの永続状態にはしません。実行時のクライアント条件は、後続の境界（アクション開始時など）で最新の在席世代番号や接続状態から再確認します（IB §10）。
- **期待される現在の前提条件**: 現在のタスクリビジョン、委任の有効性、ワークスペースの有効性、方針指示の前提条件。クライアント依存のタスクであれば、在席世代番号、現行接続状態、およびデバイスの可用性を照合します。
- **キャンセルと安全保留**: 中断要求の受理と、実際の処理遂行の停止、および外部アクションの停止完了は別個の事実です。中断後に遅延して届いた結果は、`RecordedToOriginalOnly` として元の過去記録への保存に留めます。後続のキャンセルスライスが durable な cancel marker を導入した場合、`adopt_result` は同じ不可分な比較照合の中でその marker を確認し、遅延結果を現在 Task に採用しません（本スライスは marker の producer を作らず、placeholder 列も置きません）。
- **結果の到着 record と identity・本文・相関（本スライス）**: 結果本文の master は Task owner（作業担当）が `task_result` の 1 行として保持し、identity は Task Agent execution が final result を提出した明示的な finalization 境界で作業担当の orchestrate が発行する `TaskResultId` です。orchestrate は final result の可視化より前に `record_task_result_arrival` で `{ delegation, result, body }` を 1 回だけ durable に記録します（durable-before-visible。同じ identity の再 arrival は本文行を増やさず、本文・委任・依拠リビジョンの不一致は技術的エラー）。provider 出力・Agent の自己申告・結果本文の到着は、外部作用の成功・Task の採用・Task の完了のいずれの証拠でもありません。1 回の inference turn の provider 出力（`TaskAgentTurnOutcome::Produced`）は Action 要求や途中経過であり得るため final result ではなく、この到着 record に入れて execution を seal してはなりません。`adopt_result` は同じ短い `Immediate` トランザクションで、result 行の delegation（1 delegated Task Agent execution の lifetime。0..N inference turn / 0..N Action attempt / 0..1 final result）から Action owner の `action_attempt` を列挙して authoritative set（その execution が durable start した Action 試行の完全な集合）を解決し、claim の `attempt_refs` との完全一致（欠如・追加・重複のいずれも不可）を要求します。一致しない claim は完了材料にできず、不整合な単位として技術的エラー（fail closed）です（推測で stale や `WithheldByEffectFacts` に丸めません）。
- **結果の確信度と採用判定**: エージェントの自己申告を外部作用の成功証拠として過信してはなりません。外部作用の確信度は実行・拡張担当が記録した確定事実（fact）を参照・集約し、作業担当側で勝手に更新してはなりません。claim が authoritative set と完全一致した上で、依拠試行が 1 つでも `ConfirmedSuccess` でない場合（`Unknown` / `ConfirmedFailure`）は現在 Task を完了せず、到着 record と result-local 相関だけを残します。これに加えて、`Completed` への CAS と同じ短い不分区間で **Task-wide completion barrier**（同じ `TaskId` に属する全 durable `ActionAttempt`（全 revision・全 delegation。steering 前の古い revision の started Action を含む）のうち `certainty = Unknown` の集合が空であること）を検証します。1 件でも `Unknown` が残れば現在 Task を完了せず、`WithheldByEffectFacts { attempts: blockers }`（blockers = authoritative set の `ConfirmedSuccess` 以外 ∪ Task-wide の `Unknown`。集合として重複を 1 回に畳む）を返します。barrier の入力は `action_attempt` の durable facts（`task_id`・`task_revision`・`delegation_id`・`certainty`）だけで、timestamps・delegation liveness・ephemeral agent alive flag・結果本文・自己申告・caller の `attempt_refs` は使いません。result-local authoritative set は「この result が何に依拠したか」を証明する相関であり、`task_result_attempt` に刻印するのはその検証済み集合だけです。barrier が見つけた別 delegation / 旧 revision の試行は `task_result_attempt` へ追加しません（その Action が result の dependency であるとは限らないため）。cross-delegation / 旧 revision の `ConfirmedSuccess` / `ConfirmedFailure` は barrier だけを理由に完了を block しません（`ConfirmedFailure` が result-local set に含まれる場合だけ既存規則で block します）。in-flight の inference attempt と、final result を持たない unsealed delegation の存在は barrier に含めません（started Action が無い、または started Action がすべて settlement 済みなら完了を block しません。完了 commit 後は terminal gate が新しい AU14/AU5 を拒否します）。authoritative set が durable に空である場合（その execution が Action 試行を 1 つも開始していない場合）に限り、Action 無しの結果として採用判定へ進めます（claim が空だから Action が無かった、と推定してはなりません）。`Unknown` は新しい客観的証拠が実行・拡張担当側で確定するまで保持します。authoritative set の membership は final result の到着（seal）時点で固定され、seal 後に AU14/AU5 が新しい開始を拒否するため増減しません。seal 後に `Unknown` が客観的証拠で `ConfirmedSuccess` へ進展した場合、同じ result の採用判定を再評価してよく、membership は seal 時点のままです（set membership と certainty update を混同しません）。
- **永続化とコミット**: タスクの新規作成は、`task + task_revision + 初期 task_context_entry` と、関連付けを確定した場合の `workspace_assoc` を不可分に永続化してから外部へ可視化します（durable-before-visible）。方針変更は、新リビジョンと新コンテキストを不可分に進めます。結果の到着 record は、`task_result` の 1 回だけの insert（`body` 1 行、`adopted_revision = NULL`、`task_id`/`task_revision` は delegation 行から写す）として final result の可視化前に確定し、同じ不分区間でその delegation を seal します（durable invariant: 1 delegation につき final `task_result` は最大 1 行。同じ `TaskResultId` の retry は冪等、同じ delegation の異なる `TaskResultId` は 2 つ目の final result として受理せず fail closed）。採用と完了確定は別の短い `Immediate` トランザクションで、result 行の存在確認、delegation（execution lifetime）からの authoritative set 列挙と claim の完全一致、確定度の読み取り、Task-wide completion barrier（同じ `TaskId` に属する全 revision / 全 delegation の Action 試行に `Unknown` が無いこと）の検証、`task_result.adopted_revision` の刻印、`task_result_attempt` の相関刻印、`task.progress` の terminal への CAS を行います。現在リビジョン不一致・terminal・cancel marker ありの場合は、到着 record に留めて現在 Task を変更しません。また、`create_delegation`・`forward_steering`・推論試行 claim（AU14）・Action 開始（AU5）はそれぞれの同一トランザクションで `task.progress` の非 terminal を必須とし、AU14/AU5 はさらに delegation が seal 済みでないこと（その delegation の `task_result` 行が存在しないこと）を同じ不分区間で必須とします。terminal は書き込み・開始ともに行いません。同一 `TaskId` に対する方針指示、委任受付、完了確定、結果採用は、同期区分 SD-Task によって厳格に逐次化します（CCT §4）。
- **Task progress / lifecycle（本スライス）**: Task revision（目的・指示の steering）と `TaskProgress` は別の軸です。`create_task` は `Started` で初期化し、`create_delegation` は `Started → InProgress` を同じ AU3 トランザクションで進め（end-to-end の初回実装はこのスライス）、`adopt_result` は採用成立時に、同じ不分区間で Task-wide completion barrier（同じ `TaskId` に属する全 revision / 全 delegation の Action 試行に `Unknown` が無いこと）を検証した上で `progress` を非 terminal から `Completed` へ CAS します。terminal 状態は吸収的で、terminal からの遷移は `RecordedToOriginalOnly`（結果）に留まります。`Failed` は作業担当が確定した terminal failure のみを表し、provider の一時障害・`NotSent`・Action `Unknown`・`WithheldByEffectFacts` を写しません（それらを terminal failure と確定できる producer は後続スライスが追加します）。start 要求から結果が返るまでの推論ターン自体は lifecycle を変更しません。**terminal 後の新規委任・steering・推論試行 claim・Action 開始の拒否（admission gate）は本スライスの契約であり**、AU3/AU4/AU14/AU5 の比較と同じ不分区間で `task.progress` が非 terminal であることを必須とし、terminal は専用の domain outcome（`TaskTerminal`）で書き込み・開始ともに行いません（cancel marker と cancel-specific gate は後続スライスが同じ比較点へ producer と対で追加します）。execution seal はこれとは別の gate であり、Task が non-terminal（InProgress）のままでも seal 済み delegation の AU14/AU5 は拒否されます（`ExecutionSealed`）。逆に Task が terminal なら seal の有無に関わらず `TaskTerminal` です。seal は Task の terminal への遷移を意味せず、Task terminal は execution seal の前提でもありません。本スライスが固定するのは progress の吸収性、terminal の admission gate、execution seal、terminal 後に結果を現在へ採用しないことです。
- **Task Agent の推論開始（K-E との接続）**: 委任された一時エージェントの推論は、依拠タスクリビジョンの前提を推論試行の確定（attempt claim）と同一の短いトランザクションで照合してから開始します（照合条件は K-E。現在の `task.progress` が非 terminal であることに加え、その delegation が seal 済みでないこと（その delegation の `task_result` 行が存在しないこと）を含みます）。照合に失敗した場合は `TaskPremiseStale`（タスク前提の不一致。terminal と execution seal を含む）または `Stale`（同意・認証情報の不一致）として送信前に不受理とし、provider への I/O を行いません。作業側は再読込で delegation の final result を検出した場合、`StaleTaskRevision` ではなく `ExecutionSealed { delegation }` として返し、progress の terminal を検出した場合は `TaskTerminal { task, progress }` として返します。確定済みの試行は、その後の方針指示（steering）によって開始を取り消されません（遅延結果は元リビジョンへの記録に留め、現在のタスクへ自動採用しません）。委任レコードの存在を実行中・生存の証拠にしてはなりません（CI §5.3）。
- **Task Agent の入力と scrub（本スライスの範囲）**: このスライスの論理入力は依拠リビジョンの採用目的本文（`task_revision` snapshot）であり、`SecretScrubber` の出力（`ScrubbedText`）だけを推論境界へ渡します。ハーネス自ら `ScrubbedText` を構成してはなりません（構成の封止は Issue #1530 で追跡）。採用指示の本文は History が原本のまま保持し、参照先レコードの bounded read を持つ producer が同じ設計変更で解決を追加します（本スライスは本文を複製せず、指示本文も送信しません）。ワークスペース範囲・ファイル内容はこの推論経路では送信しません（Action スライス）。scrub の失敗は fail closed の技術的失敗であり、原文を送信・保存・ログ出力しません。Task Agent は `admit_task_agent` のみを呼び、`admit_dialogue` / `admit_learning` を代用してはなりません。
- **依存性の反転（Inversion）**: 保全・消去等の他ドメインのデータ（`HoldConditionRef` や `RestoreGeneration` 等）は、クレート境界では受け入れ側のドメイン（作業、推論、実行等）が定義する前提型として受け取ります。他ドメインの固有型を直接インポートしてはなりません（CM §4.3）。
- **IPC 通信**: ホスト内部で完結します。クライアントへは進捗や結果の必要な表示情報のみを伝達し、タスクのマスターデータそのものは送信しません。

```rust
// Task Agent の推論ターン（作業側が定義する port。Host 結合ルート apps/ene-core が
// InferenceExecutor へのアダプターとして実装する。作業側は権限・認証情報の具象型を参照しない）。
struct TaskAgentTurnPremise {
    delegation: DelegationId,        // 実行する委任。行の存在は生存の証明ではない
}
struct TaskAgentInferencePremise {   // port 入力。prompt は SecretScrubber の出力のみ
    delegation: DelegationId,
    task: TaskRef,                   // 依拠タスクリビジョン
    prompt: ScrubbedText,
}
enum TaskAgentInferenceOutcome {
    Produced {                       // 1 回の inference turn の provider 出力。final Task result ではなく、execution を seal しない
        output: TaskAgentOutput,
        adoption_consent_current: bool,
    },
    StaleTaskPremise,                // タスク前提の不一致（リビジョン前進・terminal・execution seal・行欠如を含む）。provider へ送信していない
    NotSent(TaskAgentNotSent),
}
struct TaskAgentOutput;              // provider 出力本文。Debug では伏字化し、アクセサ経由でのみ読む
enum TaskAgentNotSent { SetupIncomplete, NotInAllowlist, ConsentStale, OverLimit, EvaluationConsumed }
enum TaskAgentInferenceError { InferenceUnavailable { reason: String } } // 技術的失敗（本文・秘密を含めない）
trait TaskAgentInference: Send + Sync {
    async fn infer(&self, premise: TaskAgentInferencePremise)
        -> Result<TaskAgentInferenceOutcome, TaskAgentInferenceError>;
}
enum TaskAgentTurnOutcome {
    Produced {                       // 1 回の inference turn の provider 出力。Action 要求や途中経過であり得る。
                                     // それ自体は Task result でも execution seal でもない（finalization は別境界）
        delegation: DelegationId,
        task: TaskRef,               // 依拠タスクリビジョン
        output: TaskAgentOutput,     // provider 出力本文。Debug では伏字化し、アクセサ経由でのみ読む。
                                     // durable 正本は final result が task_result に書かれた 1 行だけ
        adoption_consent_current: bool, // ネットワーク待機後も同意が成立していたか
    },
    TaskTerminal { task: TaskId, progress: TaskProgress }, // Task が terminal のため claim を拒否。provider へ送信していない
    ExecutionSealed { delegation: DelegationId }, // delegated execution が final result で seal 済みのため claim を拒否。
                                     // provider へ送信していない。Task lifecycle の terminal とは別（Task は InProgress のままであり得る）
    StaleTaskRevision { current: TaskRef }, // 依拠リビジョンが前進済み。provider へ送信していない
    MissingTask { task: TaskId },
    MissingDelegation { delegation: DelegationId },
    NotSent(TaskAgentNotSent),       // setup 不足・許可リスト外・同意失効・入力上限・利用済み評価
}
// `TaskAgentNotSent` / `TaskAgentInferenceError` は推論側 `NotSentReason` / `InferenceTechnicalError`
// と同じ意味の語彙を作業側に写したもの。`StaleTaskPremise` は作業側が委任・現在のタスク・
// delegation の final result（seal）を再読込し、委任が不在なら MissingDelegation、タスクが不在なら
// MissingTask、final result が durable なら ExecutionSealed、progress が terminal なら
// TaskTerminal { task, progress }、タスクリビジョンが前進していれば StaleTaskRevision { current } へ写す
// （判定の所有は作業担当に残る。terminal と seal を revision stale へ丸めない）。
// final result を提出する finalization 境界（TaskAgentResultArrival）は `Produced` とは別であり、
// orchestrate がそこで TaskResultId を発行して AU15a で本文を durable 化してから採用判定へ進む。
// どの provider 出力が final かを決める tool loop の低レベル protocol は本スライスでは固定しない
// （intermediate output を AU15a へ入れて execution を seal してはならないことだけを固定する）。
```

### H-B Experience 提出 → 形成判断（個体調整・作業 → 認識・学習）

```rust
struct ProposeExperienceCandidate {
    experiencer: CompanionId,        // 経験を積んだパートナー個体
    source_range: SourceRangeRef,    // 元となった会話やタスクの大まかな範囲
    source_kind: ExperienceSourceKind, // 対話／タスク／ツール実行／観測／交流などの種別
    task_ref: Option<TaskRef>,       // タスク由来である場合はそのリビジョン前提
    delegation: Option<DelegationId>,
    intended_use: IntendedUse,       // 返答／学習候補／ルーティング／権限解釈／タスク判断などの用途意図
    // 生の本文や詳細ペイロードの無駄な複製を要求しない。参照によって元データを辿れること。
}

enum FormationDecision {
    Formed { summary: SummaryId },   // 要約証拠（Summary evidence）を保存し、少なくとも1件の学習変更を適用した
    NoChangesApplied,                // コミット前照合の敗北、対象欠落、スコープ不一致、既存と同一、リビジョン枯渇等により何も保存しなかった
    DeferredForContext,              // 文脈情報が不足しているため保留（再提出時に新しい別経験として扱わない）
    DeclinedAsNoEndValue,            // 長期保存する価値がないと判断（すべての経験を保存する義務はない）
}
```

- クライアント識別子、会話ラウンドID、在席世代番号の細かな対応関係は、現在の開発ステージにおける経験形成では消費しないため、インターフェースには含めません（必要となるステージで再導入します）。
- 個人データ削除中や保存禁止制約による保留（`HeldByErasureOrConstraint`）は、現在のステージの形成処理では発生させず、削除や制約を本格的に扱うステージで安全に再導入します。
- **開始責務と判断権限**: 個体調整および作業担当が候補を提出し、認識・学習担当がその保存価値、知識形成、更新、統合、および将来の想起必要性を判断します。タスク固有の一時的な情報を自律学習へ昇格させるかどうかは、学習担当が独立して判断します。
- **保持すべき情報**: 経験の発生由来の種別、対象パートナー、関連タスク、委任関係、期待される将来の利用用途。
- 自律学習とタスク管理を混同してはなりません。ユーザーへの応答生成が完了したことと、バックグラウンドでの学習データ更新がすべて完了したことを同一視してはなりません。

### H-C 会話による訂正（個体調整 → 認識・学習）

```rust
struct ProposeCorrectionCommand {
    target: LearningId,              // 訂正対象の学習項目
    expected_revision: LearningRevision, // 期待リビジョン（boundary token）
    new_experience: ProposeExperienceCandidate, // 訂正の根拠となる新しい経験
    temporal_kind: TemporalKind,     // 最初から誤っていたのか（InitiallyWrong）、途中で変わったのか（ChangedSince）の区別
}

enum CorrectionOutcome {
    CorrectedAs(LearningRevision),   // 新しいリビジョンとして安全に訂正（過去の有効性履歴も保持）
    StaleTarget { current_revision: LearningRevision },
    HeldByErasureOrConstraint(HoldConditionRef),
}
```

- 個体調整担当がユーザーとの対話の中で「それは違うよ」という訂正発言を受け取ったとしても、それだけで学習状態を直接書き換えてはなりません。どの学習項目をどう変更すべきかという意味論的な判断は、認識・学習担当が行います。また、過去の発言ログそのものを現在の認識に合わせて改ざんしてはなりません。

### H-D Learning 参照・由来説明（query + fact）

```rust
struct LearningQuery {
    requester: LearningConsumer,     // 個体調整／作業／共有観測（ルーティング範囲限定）などの利用元
    purpose: LearningUsePurpose,     // 応答生成／タスク判断／ルーティング／由来説明などの用途別目的
    scope_need: ScopeNeedRef,        // 要求するスコープ範囲（パートナー専用／全体共有）
    current_constraints: ConstraintSnapshotRef, // 現在のスコープ制限、保存禁止フラグ、消去保留条件の写し
}

struct LearningView {
    learning: LearningId,
    revision: LearningRevision,
    scope: LearningScope,            // パートナー専用（Companion）／全体共有（Global）
    grounds: SummaryGroundsRef,      // 要約から知識形成・改訂への対応関係（本文そのものは複製しない）
    // 全体共有の現在データから、他パートナーの私的な過去全文を勝手に辿らせてはならない。
    // 取得したデータのコピーであっても、現在の制約に従う必要がある。
}
```

- クエリによる参照に成功したことは、後続の外部送信や他者共有の許可を意味しません。全体共有（Global）の知識本文が利用可能であっても、その根拠となった特定の対話ログ全文などの私的情報を外部へ漏洩させてはなりません。また、遅れて届いた知識を「到着が最後だから最新の根拠である」と誤認してはなりません。

### H-E scope 変更

意味論的な判断と、システム全体への制約の強制適用を分離します。特定のAPIの完成を急ぐのではなく、インターフェースの責任境界を明確に分けることが重要です。

```rust
struct ProposeScopeChangeCandidate {
    learning: LearningId,
    expected_revision: LearningRevision,
    proposed_scope: LearningScope,
    shared_content: SharedContentRef, // 全体共有に適した内容と、必要な背景情報の要約
    non_share_intent: Option<NonShareIntentRef>, // オーナーが明示的に非共有を望んでいる意図（存在する場合は最優先）
}

enum ScopeDecision {
    ScopeChanged(LearningRevision),
    KeptAsCompanionScope,            // 判定に迷う場合や不明な場合は安全のためパートナー専用に留める
    DeniedByExplicitConstraint,      // 明示的な保存禁止や非共有設定により変更不可
    StaleTarget { current_revision: LearningRevision },
}
// 決定が下された後のシステム全体への迂回不能な適用は、権限・制約担当と各利用箇所の協調によって行われます（K-B および第10節の保留参照）。
// 認識・学習担当自身が、権限の強制執行者になるわけではありません。
```

- 重要度が高いという理由だけで安易に全体共有（Global）にしてはなりません。また、パートナーの個別削除をきっかけとして、データを消去から逃すために自動で全体共有へ昇格させてはなりません。

### H-F Observer 向け限定 routing 文脈の供給

タスク管理と自律学習を混同せず、タスク文脈の参照は既存の個体調整と作業担当の協調を通じて行い、共有観測担当へタスクの広範な管理権限や無制限なアクセス権を移してはなりません。

```rust
struct RoutingContextOffer {
    source_owner: RoutingSourceOwner, // 会話履歴・個体文脈＝個体調整 / 記憶・学習＝認識・学習 / タスク文脈＝作業
    target_companion: CompanionId,
    purpose: RoutingPurpose,         // ルーティング判定の用途に厳格に限定
    constraint: RoutingConstraintRef, // 元データの制約 ＋ 割り当て専用の送信条件
    selection_premise: SelectionPremiseRef, // 選択を行った時点の前提条件
    // 元のデータに従属する一時的な派生表現であり、要約マスターや新規学習データ、全体共有データではない。
}
```

- パートナーのプライベートな情報を不用意に全体公開したり、他のパートナーへ勝手に共有したりしてはなりません。文章の要約やフォーマット変換によって、元データに課されていた制約を消滅させてはなりません。スコープの変更、同意の失効、個人データの削除要求が発生した場合は、現在処理中の一時的な派生データにも直ちに反映します。

### H-G 未伝達の登録・報告状況更新

```rust
struct RegisterUndeliveredFact {
    companion: CompanionId,
    source: UndeliveredSource,       // TaskRecord(TaskId) または ActivityRecord(ActivityId)
    round: Option<RoundId>,
    presence_generation: PresenceGeneration,
    restore_generation: RestoreGeneration,
}

enum ReportStatusTransition {
    PendingToSummarized,
    SummarizedToPresented,
    MarkedPresentationUnknown,       // 画面に提示されたか不明な状態を正確に保持（勝手に完了とみなさない）
    StaleSource,                     // 元の記録が通常の保持期間経過によって失われた場合の整合性確認用
}
// ネットワークの接続成功、表示データの送信完了、タスクの完了だけで「報告完了」にしてはなりません。
// 端末の画面上に実際に提示されたという観測事実に基づいて状態を更新します。
```

- マスターデータは個体調整担当が保持します。タスク由来の未伝達情報であれば、タスク記録への参照を保持します。ユーザーへの報告用要約は単なる派生データです。永続化は永続化グループB（`undelivered`）の不可分トランザクションで行います。

### H-H Schedule 設定・到来・各回対応

```rust
struct CreateScheduleCommand {
    owner_selection: OwnerSelectionRef,
    assignee: AssigneeRef,           // タスク側前提（CompanionId を直接インポートしない）
    content: ScheduleContentRef,     // 実行内容・初期ワークスペース入力
    timezone: ScheduleTimezoneRef,   // スケジュール作成時のタイムゾーン（勝手に暗黙変換しない）
}

struct ScheduleOccurrenceFact {
    schedule: ScheduleId,
    occurrence: ScheduleOccurrenceId,
    status: OccurrenceStatus,        // スキップ（Missed）／開始（Started）／契約により中断（CancelledAsTaskContract）
    started_task: Option<TaskId>,    // 各回の実行は、独立した新しいタスクとして発行される
}
// スケジュールの作成依頼そのものを、永続的な特別実行権限トークンとみなしてはなりません。
// 各回の実行タイミングにおいて、その時点の最新の権限制約を改めて再評価します。
```

## 5. Permission / Credential / Inference / Execution クラスタの interface semantics（K-A〜K-K）

「ひとつの汎用セキュリティミドルウェアを通ればすべての処理が成立する」という安易な設計を採用してはなりません。権限評価、経路解決、プロバイダ送信、実作用の実行、事実記録の各境界を明確に分離します。また、各種認証情報の秘密値そのものを、通常のインターフェースの戻り値として不用意にシステム内へ返してはなりません。

### K-A Owner 意図 → 制御変更

```rust
struct ProposeControlChangeCommand {
    origin: OwnerIntentOrigin,       // 会話上の発言由来か、管理画面の直接操作かの区別 ＋ 引用情報との対応
    kind: ControlChangeKind,         // 1回限りの承認／将来にわたるルール／プロバイダ利用同意／利用枠上限変更などの種別
    target_scope: ControlTargetScopeRef, // 対象とする活動・目的・担当パートナー・タスク／委任・クライアント／ラウンド等の範囲
    relied_intent: ReliedIntentRef,  // 根拠として依拠したオーナー自身の明確な意図やルールの対応関係
}

enum ControlChangeDecision {
    AppliedAsOneTimeApproval,        // 現在の明確な依頼に対する、1回限りの実行承認として適用
    NeedsClarification,              // 要求が曖昧、矛盾がある、過度に広範である、または重大な影響があるため確認が必要
    DeniedByBoundary,                // 恒久的な拒絶設定、常に確認（Always ask）設定、機能上限（Capability）の境界に抵触するため拒絶
}
```

- **開始責務と判断権限**: 入出力・提示担当、および個体調整・作業担当がオーナーの意図を伝達し、権限・制約担当が変更の可否を最終判断します。LLMの出力、学習データ、キャラクター設定、スキル、外部コンテンツなどは単なる参考材料に過ぎず、オーナー自身に由来する明確な管理意図との紐付けが確認できない限り、制御設定（control plane）を変更することはできません。また、ルールが保存されたこと自体を外部アクションの自動トリガーにしてはなりません。
- ルール保存用の識別子型（`RuleId`）、`StoredAsRule` の判定結果、および K-B における `rule_revision` の前提フィールドは、ルール保存機構を本格導入する開発ステージにおいて安全に再導入します。

### K-B 現在の利用可否の照合（live authorization check）

過去の判断記録と、現時点で有効な「生きた実行許可」を厳格に区別します。過去に保存された許可フラグ、委任時にコピーされた権限、事前の一括判定結果、バックアップから復元されたルール、対話文脈内の許可発言、短期間のキャッシュ判定などを、現在の実行許可として安易に流用・再利用してはなりません。

```rust
struct CheckLiveAuthorizationQuery {
    candidate: ActionCandidateRef,   // 目的・対象・データ・送信先・外部作用の解釈内容（K-H の候補との対応）
    relied_evaluation: PermissionEvaluationId, // 依拠した過去の判断記録との対応（存在する場合）
    task: Option<TaskRef>,           // タスクのリビジョン前提
    delegation: Option<DelegationId>,
    workspace: Option<WorkspaceAssocId>,
    presence: Option<PresenceCheckRef>, // クライアント依存の処理であれば、申告された在席世代番号 ＋ 現行接続の前提
    consent_revision: Option<ConsentRevisionExpectation>,
    cap_context: CapCheckContextRef, // 費用・リソース・並列数・反復回数の上限照合用コンテキスト
    hold_context: HoldCheckContextRef, // 失効・停止・消去中・復元保留などの安全保留の照合用コンテキスト
}

enum LiveAuthorizationDecision {
    AllowForThisUse(PermissionEvaluationId), // 今回の具体的な利用に対する確定（再利用可能な包括許可ではない）
    Deny(DenyReasonRef),
    AskOwner(OwnerQuestionRef),      // オーナーの判断が必要なため実行を保留して待機
    WaitForCondition(WaitConditionRef),
    NeedsRevalidation(NeedsRevalidationRef),
}
```

- 各処理の開始直前に、現在の最新条件を実際の操作対象へ厳格に適用します。適用の執行責任は実行・拡張担当（外部作用）、推論担当（プロバイダ送信）、および各参照・保存箇所に残り、許可の意味論そのものの管理は権限・制約担当が担います。なお、無関係な軽微な変更が生じるたびに毎回再承認を要求するのではなく、重大な意味の変化があったかどうかに基づいて再評価の要否を判断します（K-H）。
- 環境設定の完備状態（利用同意の記録、認証情報の登録、トークンの有無など）の確認は、推論受付ゲート（K-E）が唯一の責任者であり、本クエリには設定状態のチェックを含めません。本リアルタイム照合（live check）は同意状態そのものだけを判定し、セットアップ不足を理由にして独立に拒絶することはありません。

#### K-B.1 Stage 4 の Workspace 内ファイルシステム Action に対する最小の Action 評価

推論用の `PermissionEvaluationId` / `EvaluationTracker` を Action に流用・偽装してはなりません。権限・制約担当は、Action 用の候補・現在前提・単一使用の評価を独立に持ちます。

```rust
// 権限・制約担当が所有する。Task 側の対応関係は前提として受け取り、許可の意味判断は権限側に残す。
struct ActionUseCandidate {
    delegation: RawId,             // 依拠する委任対応
    task: RawId,                   // 依拠タスク
    task_revision: RevisionInner,  // 依拠タスクリビジョン（(task, revision) の対）
    workspace: RawId,              // 現在の workspace_assoc
    operation: ActionKind,         // List | Read | Create | Edit（この段階の closed world）
    resolved_target: String,       // 実行直前に解決された canonical な実対象（binding）
}
// 呼び出し側が Task 側の永続状態から再読込した現在の前提。
// durable copy を生きた許可にせず、判断のたびに現在値と突き合わせる。
struct CurrentActionPremise {
    delegation: RawId,
    task: RawId,
    task_revision: RevisionInner,
    workspace: RawId,
}
struct ActionPermissionEvaluationId(RawId); // 推論用 PermissionEvaluationId とは別の型
enum ActionKind { List, Read, Create, Edit }
enum ActionAuthorizationDecision {
    AllowForThisUse(ActionPermissionEvaluationId), // 今回の利用限り。候補 fingerprint に束縛し、single-use
    Deny(ActionDenyCode),
    NeedsRevalidation,
}
enum ActionDenyCode {
    NotInAllowlist,   // 明示的な許可アームの外（Delete/Execute 等を producer なしで通さない）
    PremiseMismatch,  // 候補と現在前提の不一致（無効化・再評価）
}
class ActionEvaluationTracker { /* mint / consume。推論用 EvaluationTracker とは別実体 */ }
fn authorize_action_use(
    candidate: ActionUseCandidate,
    current: &CurrentActionPremise,
    tracker: &mut ActionEvaluationTracker,
) -> ActionAuthorizationDecision;
```

- **明示的な許可アーム（default-allow の禁止）**: この段階の許可は、委任された Task Agent の Workspace capability に対する `List / Read / Create / Edit` の明示的な組です。候補の `(delegation, task, task_revision, workspace)` が `CurrentActionPremise` と一致し、操作が明示アームに含まれる場合だけ `AllowForThisUse` を返します。Delete/Execute、ルール・Deny・Always ask の producer は、この関数の明示的な拡張として追加し、既定許可で通してはなりません。
- **binding と単一使用**: 評価 ID は候補 fingerprint（delegation・task・task_revision・workspace・operation・resolved_target）に束縛し、`consume` は一致する fingerprint での一度だけ成功します。同じ ID を別の対象や操作に使い回すことはできません。
- **AU5 start への接続**: 実行・拡張担当は対象を解決した後に `authorize_action_use` を呼び、`AllowForThisUse` の場合だけ評価 ID を consume してその opaque な `RawId` に落とし、`AttemptCommitPremise.relied_evaluation` として AU5 の挿入トランザクションへ渡します。挿入トランザクションは同じ不分区間で委任・依拠タスクリビジョン・現在のタスク・現在の workspace_assoc・委任スコープを再照合するため、評価の前提が動いた場合は開始されません。永続化された `relied_evaluation` と試行行の対応関係（操作・実対象・委任・依拠 TaskRef）が、その判断の durable な binding です。同じ評価 ID での二度目の開始は技術的エラーとして拒否し、二重開始を許しません。
- **判断記録との区別**: `permission_evaluation` の評価ログ行（D2）、ルール・Deny・毎回確認の永続状態、オーナー確認（AskOwner）は、それらの producer を持つスライスが同じ判断点に追加します。このスライスは代役の許可・placeholder 列を置きません。

### K-C 秘密利用（認証用途への供給）

呼び出し元へ秘密情報そのものの所有権を渡さない設計を徹底します。認証情報の参照（ハンドル）を持っていることと、その秘密情報を自由に読み出せることは全く別です。

```rust
struct RequestAuthenticatedUseCommand {
    credential_ref: CredentialRef,   // 秘密情報を含まない、用途別の不透明な参照ID
    connection: ConnectionNeedRef,   // どの接続先や用途で必要としているかを示す参照情報（秘密値は含まない）
    operation: OperationKind,        // 認証を伴う具体的な操作の種別
    // LLM が生成した単なる参照文字列だけでは利用可能になりません。
    // 設定済みの接続先、利用用途、有効期限の照合が不可欠です。
}

enum AuthenticatedUseOutcome {
    UsedWithinScope,                 // 必要な範囲内でのみ安全に秘密情報を利用（秘密値そのものは返却しない）
    NeedsReauthentication,           // 認証情報の失効、不足、または無効化
    DeniedByConstraint,              // 保存禁止、失効、安全保留などにより利用不可
    StaleReference,                  // 参照情報と現在の保管庫・用途との不一致
}

// 供給側の重要な規約：
// 認証情報の秘密値そのものを、モデルのコンテキスト、LLMが生成したツール引数、通常の結果データ、
// UI画面、対話履歴、要約、学習データ、タスク結果、通常ログ、監査ログ、デバッグ出力、
// バックアップアーカイブへ絶対に流出させてはなりません。
// 接続先からのエラー応答や結果の受け取り・保存・画面表示の各箇所も協調し、秘密値の非露出を維持します。
```

- 秘密情報本体は、OS の資格情報マネージャー等の独立した保護領域（E側）に安全に保管します。データベース側には秘密を含まない参照情報のみを保持します（PR グループK）。Rust のインターフェース上でも `SecretValue` 型をパブリックな戻り値として返してはなりません。必要な認証用途に対して「利用させる（認証処理を代行する）」ことはあっても、値そのものは認証秘密情報の境界内に厳重に閉じ込めます。たとえば `with_credential(handle, |opaque| ...)` のようなスコープ限定の安全なインターフェースや、接続担当者への直接供給とし、汎用的な `get_secret() -> String` のような安易な取得関数は一切設けません。

### K-D Provider 割当解決

一度解決された通信経路の情報を、恒久的に利用可能なプロバイダ割り当てのマスターデータとみなしてはなりません。

```rust
struct ResolveAssignmentQuery {
    consumer: UsageConsumer,         // パートナーの主思考／観測専用／タスクエージェント（継承）などの消費主体
    capability: CapabilityKind,
    current_consent: ConsentExpectationRef, // 期待される同意リビジョン（expected consent revision）
}

struct ResolvedRouteCandidate {
    route: ProviderRouteRef,         // 導出された経路の候補（決定権威そのものではない）
    consent: AssignmentConsentId,    // 根拠として依拠した同意情報の対応関係
    capability_gap: Option<CapabilityGapRef>, // 能力不足が存在する場合は、実行前にその差分を明示する
}
// 最終的な利用可否の確定は、K-B のリアルタイム照合において最新の条件と突き合わせて行います。
// プロバイダの登録や初期認証が成功したことだけで、無条件の利用権限を成立させてはなりません。
// また、実際の送信処理（K-E）の段階で、確定済みの前提条件を無駄に再照合してはなりません。
```

- 観測処理（Observer）は専用の割り当てを解決して動作し、パートナー側の設定を勝手に上書きしたり、利用同意の選択や合成を独自に行ったりしてはなりません。
- Task Agent の割り当ては委任元 Companion の割り当てを継承します。消費主体の語彙（design 上の `UsageConsumer`、実装の `ConsumerKind`。owner は権限・制約）に Task Agent 継承の消費主体（`TaskAgent`）を追加し、委任元の推論経路が属する既存 Capability（`Dialogue`）と Task Agent のターン専用の Purpose（`TaskAgentTurn`）の組 `(TaskAgent, Dialogue, TaskAgentTurn)` を明示的に許可リストへ追加します（default-allow は禁止）。実装段階では capability 単位の同意レコードが委任元の現在の経路を示すため、解決は受付ゲート（K-E）が委任元が依拠する現在の Capability 同意から live に行い、委任行や推論試行に独立した割り当て identity を保存しません。将来 Host 既定 → コンパニオン上書きの多段解決・per-companion 次元を導入するスライスが、その解決規則（`assignment_consent` / `AssignmentConsentId` は設計上既に権限・制約の所有）と同じ設計変更で追加します。purpose/consumer 単位の Deny・Always ask を導入するスライスは、この組も明示的な評価対象に含めます。

### K-E 推論の実利用ごとの成立・送信

最初のプロンプト送信、再送、補助的な推論呼び出し、対話の継続に伴う一連の送受信の各ステップについて、それぞれを独立した「実際の利用」として厳格に成立させます。なお、フォールバック機構（K-F）は、マルチプロバイダおよび費用上限を本格導入するステージ16で再導入します。

```rust
struct AdmissionRequest {
    candidate: InferenceUseCandidate, // 消費主体・利用用途・論理コンテキスト・要求能力の対応関係（写し）
    // 利用同意や認証情報の前提条件は、受付ゲート（admission）が読み取る確定材料です。
    // 秘密値そのものや内部の権限詳細を呼び出し元へ漏洩させてはなりません。
}

enum Admission {
    Admitted(Box<AuthorizedInference>), // 確定した前提情報（チケット、同意前提、プロバイダ／モデル情報、候補情報）
    Declined(NotSentReason),            // 前提不足、失効、許可リスト対象外など。送信前に安全に中断し、副作用を残さない
}

struct InferenceAttempt {
    ticket: InferenceTicketRef,
    consumer: UsageConsumer,                         // 消費主体（closed world は許可リストが所有。`TaskAgent` 等）
    capability: CapabilityKind,
    purpose: PurposeKind,                            // 利用目的（この試行が何のための利用か）
    expected_consent: ConsentPremise,               // (id, rev) をペアとして厳格に保持
    expected_credential_set: CredentialSetRevision, // 秘密情報が適切に除外された認証情報セットのリビジョン
    provider: ProviderRouteRef,
    model: ProviderRouteRef,
    started_at: WallClockWithTz,                     // 試行確定日時
    // Task Agent の利用では、委任の対応関係と依拠 TaskRef の前提（`(task_id, task_revision)`）を
    // 試行確定と同一のトランザクションで照合し、試行行へ durable に保持します（他の利用では不在）。
    // 結果到着・採用・確定度は試行行に持たせず、それぞれの producer が追加します。
    task_agent: Option<TaskAgentAttemptPremise>,
}

// Task Agent 試行の受け入れ側前提（推論クレートが定義する）。作業側の具象型（DelegationId /
// TaskRef）はインポートせず、不透明 ID と revision 数値の前提として受け取ります（CM §4.3）。
// マッピングは作業側の orchestrate / Host 結合ルート（apps/ene-core）だけが行います。
struct TaskAgentAttemptPremise {
    delegation: RawId,               // 委任の対応関係（durable correlation）
    task: RawId,                     // 依拠タスク（不透明 ID）
    task_revision: RevisionInner,    // 依拠リビジョン（(task, revision) のペアとして扱う）
}

enum InferenceDispatchOutcome {
    Completed {
        arrival: InferenceResultArrival,
        adopted: bool, // 待機（await）完了後の結果採用の同意が正常に成立したかどうか
    },
    NotSent(NotSentReason), // 送信前に安全に拒絶された状態（利用実績データは残さない）
}

struct InferenceResultArrival {
    ticket: InferenceTicketRef,      // 元の要求チケットおよび範囲との対応関係
    output_text: ProviderOutputRef,  // 送信表現、プロバイダ側のコンテキスト、戻り値と元要求との整合性
    usage: UsageFactRef,             // プロバイダから報告された利用量（または計測不明の区別付き）
}
```

- `NotSentReason` と試行確定の結果は、Task Agent のタスク前提不一致（`TaskPremiseStale`）を同意・認証情報の失効（`ConsentStale` / `Stale`）とは別の列挙子として持ち、技術的エラーへ吸収しません。作業側は `TaskPremiseStale` を受けて委任・現在のタスク・delegation の final result（seal）を再読込し、`MissingDelegation` / `MissingTask` / `TaskTerminal { task, progress }` / `ExecutionSealed { delegation }` / `StaleTaskRevision { current }` へ写します（判定の所有は作業担当に残ります。terminal と seal を revision stale へ丸めず、`TaskPremiseStale` に Task lifecycle の値型を import させません）。

- **送信の手順**:
  1. 受付ゲート（admission）が最新の同意・認証前提と、K-B の単一利用認可を確認して `Admission` を返します。Task Agent の admission は `(TaskAgent, Dialogue, TaskAgentTurn)` を固定した専用経路（`admit_task_agent`）のみで作り、`admit_dialogue` / `admit_learning` の代用を許しません。
  2. 試行の確定（attempt claim）が、保存された同意情報および認証情報セットとの一致を単一のトランザクションで確定した上で、トランスポート層を介して送信します。Task Agent の利用では、同じトランザクションで次の 5 条件を照合します: (1) 前提の `delegation` が `delegation` 行に存在する、(2) その行の `(task_id, task_revision)` が依拠 `TaskRef` 前提と一致する、(3) 現在の `task` 行のリビジョンが依拠リビジョンと一致する、(4) 現在の `task` 行の progress が非 terminal である、(5) その delegation が seal 済みでない（その delegation の `task_result` 行が存在しない）。(1)(3)(4)(5) の不一致と行の欠如は `TaskPremiseStale` として送信前に拒絶し(2) の不一致や部分的・不整合な行は技術的エラー（fail closed）とします。terminal を含む refusal を受けた作業側は、再読込で terminal を検出すると `TaskTerminal { task, progress }`、delegation の final result（seal）を検出すると `ExecutionSealed { delegation }` へ写します。同意・認証情報の不一致は従来どおり `Stale` で、`TaskPremiseStale` と区別します。
  3. 入力トークン上限は確定前に、プロンプト内の認証情報セット前提は試行確定と同一のトランザクションで照合します。
  4. 試行確定後のプロバイダへの非同期I/Oはロックを持たずに並行実行し、送信の瞬間に権限やルーティングを二重に検証することはありません（受付ゲートとの二重チェックによる競合を防ぐため）。
  5. ネットワーク待機（await）後に同意状態が変化して結果を採用できなくなった場合は、生成結果の採用のみを安全に破棄し、利用実績の記録は確定した試行情報に従って正しく残します。
- 環境設定の完備状態（同意の記録、認証情報の登録、トークンの有無）の判定責任は受付ゲートのみが持ち、不足があれば送信前に `NotSent` として処理します。権限側のリアルタイム照合は同意状態そのものだけを評価します。
- 「データを参照できたこと」と「外部へ送信してよいこと」を厳格に区別します。一度解決された送信先を包括的な許可とみなしてはならず、同意が不足しているからといって送信内容を勝手に削って無言で送信してはなりません。プロンプトキャッシュやセッションの再利用はパフォーマンス最適化に限定し、権限判定そのものにも独立した割り当て同意・認証用途・費用上限を厳格に適用します。

### K-F fallback 選択

```rust
struct ProposeFallbackCandidate {
    ticket: InferenceTicketRef,
    failed_route: ProviderRouteRef,
    next_candidate: ProviderRouteRef, // オーナーが事前に承認したフォールバック順序内の次候補
}

enum FallbackDecision {
    AllowedAsApprovedFallback,
    DeniedAsUnapprovedRoute,         // 未承認のクラウドへの自動切り替えを確実に防止
    NeedsRevalidation(NeedsRevalidationRef), // 用途、対象データ、プライバシー設定、認証情報、利用枠の再照合が必要
}
```

- 「料金が安いから」「機能が不足しているから」といった理由で例外を作ってはなりません。プロバイダの切り替え等によって同意の意味が大きく変化する場合は、過去の同意をそのまま引き継いではなりません。
- 本インターフェースは、マルチプロバイダおよびフォールバック機構を導入するステージ16において正式に再導入します。

### K-G 利用量の予約・確定・解放（cost reservation）

同一の利用枠の残額を、複数の処理が独立に二重消費することを許しません。また、処理中の利用量や成否不明の消費を「ゼロ」とみなしてはなりません。

```rust
struct ReserveUsageCommand {
    consumer: UsageConsumer,
    cap: CapId,                      // 利用枠ID単位（プロバイダ単位または全体スコープ）
    upper_bound: UsageUpperBoundRef, // 想定される利用上限（過小な予約による上限突破を防止）
    attribution: UsageAttributionRef, // 利用目的および送信先との対応関係
}

enum ReservationOutcome {
    Reserved(ReservationId),
    DeniedByCap(CapStateRef),
    HeldForUnknownCost,              // 費用が不明であり安全に継続できないため保留
    NeedsRevalidation(NeedsRevalidationRef),
}

struct CommitUsageCommand {
    reservation: ReservationId,
    actual: UsageActualRef,          // 確定した実測利用量（または成否不明の区別付き）
}

struct ReleaseUsageCommand {
    reservation: ReservationId,      // 未使用分の予約枠の安全な解放（すでに発生した消費や不明な消費は解放しない）
}
```

- 利用実績の元データは各利用担当者（推論、作業、実行、拡張、保全、消去）が保持し、権限・制約担当は利用枠の上限可否のみを管理します。永続化は永続化グループF（`usage_fact_*`）のトランザクションで行い、予約のコミットは短いトランザクション内の不可分な照合（SD-Cap）で行います。予約完了後の推論処理はロックなしで並行実行されます。なお、費用の計算アルゴリズム、集計期間、推定方式は実装の自由度に委ねられます。
- `CommitUsageCommand` は、システムの再起動後やエージェント停止後の再評価フローからも安全に呼び出せます。処理中にプロセスが停止して宙に浮いた予約（`Reserved`）は、元の予約記録に基づいて「成否不明」として安全に確定させ、勝手に解放（release）してはなりません（PR §6.2、CCT §9.2）。呼び出し元プロセスの生存を必須条件とせず、他者に利用事実の書き換え権限を勝手に移しません。不明となった理由と金額は、管理画面で明確に区別して表示します。

### K-H Action 候補 → 認可 → 実作用 → 結果確定

権限判定時に記述された対象の名前と、実行時に解決された実際の対象が文字列として一致していることだけで、同一の対象とみなしてはなりません。実行時の厳密な名前解決を経て、真の対象との対応関係を照合します。

```rust
struct ActionCandidate {
    principal_chain: PrincipalChainRef, // 実行主体および委任の連鎖情報
    task: Option<TaskRef>,           // タスクのリビジョン前提
    delegation: Option<DelegationId>,
    workspace: Option<WorkspaceAssocId>,
    purpose: ActionPurposeRef,       // ユーザー依頼、自発的提案、スケジュール実行、方針変更のどれに基づくか
    described_target: DescribedTargetRef, // 判定時に想定した対象の説明
    operation: OperationKind,        // 読み取り／作成／編集／削除／実行の各操作を混同しない
    data_use: DataUseRef,            // 取り扱う主なデータと利用用途（認証情報の秘密値は含めない）
    cost_risk: CostRiskRef,          // 予想費用とリスク（推定値か不明かの区別付き）
    relied_intent_rule: ReliedIntentRuleRef, // 依拠したオーナーの意図やルールの対応関係
}

// 実際の実行直前における最新条件の再確認トリガー（実行・拡張担当が適用）
// 重要な意味の変化が生じた場合にのみ再評価を行う。
struct RecheckTrigger {
    // 対象の同一性、操作種別の拡大、目的や指示の変更、タスクや委任の変更、
    // ワークスペースやスコープの変更、在席帰属の変更、依拠ルールの変更、
    // 同意やデバイス・認証情報の変更、費用やリスクの大幅な変動、安全停止や保留の発生など。
    // 無関係な軽微な変更を再評価の理由にしてはならない。
}

struct ExecuteActionCommand {
    candidate: ActionCandidate,
    authorization: PermissionEvaluationId, // K-B で得た今回限りの確定ID（包括許可のコピーではない）
    resolved_target: RealTargetRef,  // 実行直前に解決された実際の操作対象
    presence: Option<PresenceCheckRef>, // クライアント依存の処理であれば世代番号 ＋ 現行接続前提
    cost_reservation: Option<ReservationId>,
    erasure_check: ErasureConditionRef, // 実行中に発生したデータ削除要求の照合用
    restore_premise: RestoreGeneration, // バックアップ復元を跨ぐ参照を防ぐ世代タグ
}

enum ActionStartOutcome {
    StartedAsAttempt(ActionAttemptId),
    Denied(DenyReasonRef),
    AskOwner(OwnerQuestionRef),      // オーナーの指示が必要なため実行せず待機
    StalePremise(StalePremiseRef),   // タスクリビジョン、委任、ワークスペース、実対象、許可、帰属、消去、復元の条件不一致
    TaskTerminal,                    // 対象 Task が terminal（Completed / Failed）のため開始しない。書き込み・実行ともになし。
                                     // Action 側は Task lifecycle の値型を import せず、Task owner が自分の
                                     // TaskTerminal { task, progress } として意味を説明する
    HeldByGlobalHold(HoldConditionRef), // 安全保留スライスで導入
}

// 実行開始後の進捗および結果の追跡（試行ごとの CAS による並行管理）。実行・拡張担当が確定する。
struct ReportEffectFact {
    attempt: ActionAttemptId,
    stage: ActionStage,              // 受付／開始／送信／把握などの進行段階
    certainty: ActionCertainty,      // 確認済み成功／確認済み失敗／成否不明（安全に維持し勝手に変更しない）
    grounds: EffectGroundsRef,       // 結果の根拠情報（エージェント自身の自己申告を証拠にしない）
    prior_unknown: Option<ActionAttemptId>, // 再試行の場合、元の成否不明試行との対応（新試行であることの証明）
}
```

- **処理段階の厳格な分離**:
  ユーザー入力の受付、タスクの受理、認可の判定、外部作用の開始、実際の作用の把握、内部データへの保存、タスクの達成判定、オーナーへの報告完了は、すべて独立した別の事実です。これらを単一の「成功」という状態にひとまとめにしてはなりません。「確認済み成功」「確認済み失敗」「成否不明」を明確に区別し、確認が取れない事象を勝手に「未実行」や「失敗」「成功」へ改ざんしてはなりません。再実行は必ず独立した新しい試行として扱います。
- タイムアウトが発生しても、外部作用の確信度は変化しません（`Unknown` のまま保持）。遅れて成功の確認が取れた場合は、元の試行記録に対して `Unknown` → `Confirmed` のアトミック更新（CAS）を行い、現在のタスクへ結果を採用するかどうかは H-A の受付インターフェースにおいて改めて判定します。
- 永続化は永続化グループE（`action_attempt`）のトランザクションで行い、実行開始前の比較照合は短いトランザクション内で不可分に完了します（CCT §8.1、PR AU5）。
- **本スライス（Task Agent の Workspace 内ファイルシステム限定）の具体契約**:
  - **操作と request/result**: `OperationKind` は `List / Read / Create / Edit` の closed world。request の target は Workspace 相対パスのみとし、絶対パス入力は拒否します。`List` は存在する directory のみを対象とする非再帰列挙で、request は相対 dir path、result は `{name, kind: file | dir}` の列（名前昇順）です。各 direct child は no-follow metadata（`symlink_metadata` 相当）で分類し、symlink / reparse point / junction / mount entry を自動 follow しません。これらは `file | dir` に写像せず結果から除外します（存在しても List 全体は拒否しません）。`Read` は存在する regular file のみを対象とし、request は相対 file path、result は file content です。`Create` は存在しない regular file の新規作成のみとし、request は相対 file path＋body、result は作成済み marker（解決済み実対象付き）です。`Edit` は存在する regular file の上書きのみとし、request は相対 file path＋body、result は更新済み marker です。directory に対する `Read`/`Create`/`Edit`、regular file に対する `List` は拒否します。`Delete / Execute` はユーザー確認・外部拡張の producer を持つスライスが同じ設計変更で追加します。実装は List の no-follow 分類・除外に対する focused test を必須とします。
  - **authorization**: 開始の直前に K-B.1 の `authorize_action_use` が今回限りの判断を返し、`AllowForThisUse` の評価 ID を consume してその opaque な `RawId` に落とし、`AttemptCommitPremise.relied_evaluation` として AU5 の挿入へ渡します。`workspace_assoc` は作業場所の authority であり、それ自体を特定の操作の許可とみなしません。委任の `scope_copy` は依拠時点の写し（provenance）であり、開始時に照合する生きた境界は現在の `workspace_assoc` です。
  - **mount / reparse boundary**: 実際の対象の解決（`RealTargetRef`）は実行直前に行い、入力文字列の一致を同一性の根拠にしません。canonical 化とフォルダ配下の prefix 照合に加え、Workspace root の実体境界から外れる target は resolve も start もできません。Linux では `/proc/self/mountinfo` 上の入れ子 mount point を target の祖先に持つ場合と、root と target の device が異なる場合を拒否します。その他の Unix では root と target の device 一致を必須とします。Windows では root と target の volume serial number の一致を必須とします。mountinfo・device・volume の取得を含め、境界の判定ができない場合は fail closed（拒否）します。symlink / reparse point / junction は canonical 解決で外へ出るものを拒否します。`Create` の非存在 target 自体は canonicalize/stat/volume 判定できないため、存在する親 directory を canonicalize し、親と全 existing ancestor に Workspace containment＋symlink/reparse/mount/device/volume の fail-closed 照合を行い、final component が Normal な単一 leaf で未存在であることを確認して `canonical_parent.join(leaf)` を `RealTargetRef` とします。親が存在しない場合・leaf が Normal な単一 leaf でない場合・leaf が既存の場合は拒否します。publish 時の no-clobber 再確認は canonical contract に残します。実装は symlink・mount・reparse・junction の escape 拒否と `Create` の親基準解決に対する focused tests を必須とし、判定を実装できない platform・path は fail closed とします。
  - **effect facts**: 作用の確定度は実行の観測（write 後の読み戻し、List の列挙結果）からのみ記録し、エージェントの自己申告を根拠にしません。結果採用・タスク達成はこのスライスに含めません（H-A の受付インターフェースが担当）。

### K-I 拡張受入

```rust
struct RequestExtensionUseCommand {
    extension: ExtensionKind,        // MCP ツール／リソース／プロンプト、プラグイン（限定された拡張点）、MCP Apps UI
    target_action: ActionCandidate,  // 実際の外部作用との対応関係（ツールの名称だけで権能を過小評価しない）
    mcp_connection: McpConnectionRef, // 秘密を含まない接続設定・コマンド・設定の由来
    sandbox_exception: Option<SandboxExceptionRef>, // 特定のローカルMCPに限定されたサンドボックス隔離外例外（プラグインへ安易に流用しない）
}

enum ExtensionUseOutcome {
    AcceptedUnderLimit,
    DeniedAsBypass,                  // 拒絶設定の迂回、内部システムへの回り込み、自己承認とみなされるため拒絶
    NeedsRevalidation(NeedsRevalidationRef),
}
// 外部プロセス内部に対する完全な強制力や、100%確実な即時停止を保証したかのように報告してはなりません。
// 外部プロセスでの作用が確認できない場合は、成否不明として扱います。
// また、MCP Apps による画面操作は外部ツールのUIに対する一時的な入力に過ぎず、公式（第一者）の制御変更や権限承認を直接成立させるものではありません。
```

### K-J Client 依存作用の限定・移動協調

```rust
struct RequestClientBoundActionCommand {
    base: ExecuteActionCommand,      // K-H の実作用要求
    delegator_presence: PresenceCheckRef, // 委任元パートナーが現在アクティブに在席しているクライアントの前提条件
    device_permission: DevicePermissionExpectationRef,
}
// 特定のクライアント上にパートナーが存在していることは、その端末でのアクション実行許可を意味しません。
// 環境観測（ambient Observation）が有効化されていることを、操作の承認とみなしてはなりません。
// また、別の端末へ移動したからといって、旧端末で成否不明となったアクションを新端末で勝手に自動再実行してはなりません。
```

### K-K 失効・停止・保留の伝播、遅延到着・不明の扱い

```rust
struct PropagateHoldNotification {
    scope: HoldScopeRef,             // 対象範囲と保留理由（認証情報の失効、安全停止、利用枠超過、個人データ削除中、復元保留など）
    hold: HoldConditionRef,
}

struct LateArrivalAttribution {
    attempt: Option<ActionAttemptId>,
    task_revision_premise: Option<TaskRef>,
    source_range: Option<SourceRangeRef>,
    generation_tags: GenerationTagsRef, // 在席世代／復元世代／スイープ世代の区別
    arrival_kind: LateArrivalKind,   // ツール実行結果／承認応答／推論結果／利用量報告などの種別
}

enum LateArrivalHandling {
    RecordedToOriginal,              // 元のアクションやタスクの過去記録への対応付け（後続処理の自動開始は行わない）
    AdoptedToCurrentIfPremiseHolds,  // 現在の前提条件に完全に合致する用途にのみ採用
    SuppressedByErasure,             // 個人データ削除の条件に基づき、データの再保存を安全に抑止
    KeptUnknownWithDupRisk,          // 成否不明のまま保持し、二重実行リスクを明示してオーナーの判断へ戻す
}
```

- 複数の保留理由が重複している場合、1つの理由が解消されただけで他の禁止事項まで勝手に解除してはなりません。中断や失効が発生したからといって、過去の事実の正確な記録まで禁止されるわけではなく、逆に記録が完了したからといって次の利用が自動的に許可されるわけでもありません。

## 6. Presence / I/O / Observation クラスタの interface semantics（X-A〜X-H）

クライアントからの自己申告メッセージのみを根拠にして、決定権威を持つ在席（presence）を成立させてはなりません。また、共有観測のルーティング（Observer routing）と、各パートナー個別の推論処理（Companion individual inference）は、別の境界として厳格に分離・維持します。

### X-A presence 成立・移動・復帰・復旧（個体調整 → 接続・存在）

```rust
struct RequestMoveCommand {
    companion: CompanionId,
    from_client: Option<ClientId>,
    to_client: ClientId,
    reason: MoveReasonRef,           // オーナーの明示的な呼び出し／事前指示／文脈に応じた自発的移動／ホスト側の通常切断によるフォールバック／ホスト再起動後の復旧などの種別
    expected_generation: PresenceGeneration, // 期待される在席世代番号（boundary token）
    expected_state: PresenceStateExpectation,
    round_closure: RoundClosureRef,  // 入出力・提示担当および実行・拡張担当からの対話区切り、利用不能、作用不明の対応関係
}

enum MoveDecision {
    TransitioningToNew(PresenceGeneration), // 「旧端末 → 移行中 → 新端末」の安全な永続状態遷移を開始
    RejectedAsStalePresence(StalePresenceRef), // 期待した在席世代番号または状態が不一致（差し戻し）
    DeniedByConstraint,              // ペアリング未完了、デバイス無効、安全停止中、保留中などによる拒絶
}

struct PresenceAttributionFact {
    companion: CompanionId,
    state: PresenceState,            // 在席中（Present）／在席なし（NoActive）／移行中（InTransition）／停止中（Stopped）／復旧待機中（RecoveryWait）
    active_client: Option<ClientId>,
    generation: PresenceGeneration,  // 単調増加する在席世代番号（処理区間の厳密な識別に用いる）
}
```

- **判断権限と責務の分担**: 在席帰属の正式な成立は「接続・存在担当」が決定します。移動が必要かどうかの意味判断は「個体調整担当」が行い、ペアリング状態やデバイスの利用可否は「権限・制約担当」が判定します。ホストPC上で同居するローカルクライアントであっても、リモート端末とまったく同一の帰属制約を適用します。端末間を移動している移行中の過渡状態では、新旧いずれのクライアントにおいても端末依存の新規処理を開始してはなりません。また、1体のパートナーが複数端末で同時にアクティブになる「二重在席（二重アクティブ）」を厳格に禁止します。同一 `CompanionId` に対する在席切り替え処理は、同期区分 SD-Presence によって厳格に逐次化されます（CCT §4）。
- 単なるUI上の表示ヒントや復旧先候補の更新を、在席の正式な成立と同一視してはなりません。過去に保存された古い接続情報を根拠にして、現在の接続を勝手に復活させてはなりません。

### X-B Text / Voice round の帰属・区切り・提示

```rust
struct SubmitClientInputCandidate {
    companion: CompanionId,
    client: ClientId,
    claimed_generation: PresenceGeneration, // クライアント側の自己申告（決定権威ではない）
    round: RoundId,
    input_ref: ClientInputRef,       // 入力本文への参照（一時的な表現）
    trial_or_candidate: Option<ObservationCandidateOrTrialRef>,
}

enum RoundIntakeOutcome {
    AcceptedForRound(RoundId),       // 現在の在席帰属、現行の接続、実行許可、停止フラグ、保留フラグの照合がすべて正常に成立
    StaleRound(StaleRoundRef),       // 古い世代や過去のラウンドへの遅延入力（元の古いラウンドに対応付け、勝手に新しいラウンドへ付け替えない）
    HeldForTransition,               // 端末切り替え中のため新規受付を一時禁止
    NeedsRevalidation(NeedsRevalidationRef),
}
```

- `RoundClosureFact` や `ConfirmPresentationObservation`（画面提示の最終結果型）は、現在の開発ステージでは直接の送受信者が存在しないため、必要となるステージで安全に再導入します。画面提示の確認は、通信層の `PresentationStatus` からドメイン層の提示マーカー（`PresentationMark`：ラウンドIDと「提示完了／成否不明」の組）への受信変換によって行い、「メッセージを送信できたこと」と「ユーザーへの提示が完了したこと」を明確に区別し続けます。
- 会話の意味論や過去の履歴管理は「個体調整担当」が、対話ラウンドの実際の進行・画面提示・区切りは「入出力・提示担当」が、在席の成立判定は「接続・存在担当」が担います。過去のラウンドに対する入力、生成途中のテキスト、未提示のメッセージを、勝手に新しいラウンドへ引き継いではなりません。また、「LLMによる文章生成が完了したこと」を「ユーザーへ提示されたこと」と混同してはなりません。音声入力においても、単に音声が届いたことだけを根拠にして「話者の本人確認が完了した」と勝手にみなしてはなりません。
- 通信プロトコルにおける新規ラウンドの開始要求（IPC §13.1 の `SubmitTextInput.round = None`）は、入出力・提示担当が現在の前提条件を照合して新しいラウンドを発行してから、本インターフェースの `RoundId` を満たします。クライアントや通信マッピング層が勝手に業務上の識別子（ID）を発行してはならず、過去ラウンドの拒絶を新規ラウンドの自動開始へ勝手に読み替えてはなりません。

### X-C Body 表示の帰属

アバター身体（Body）の表示は、「在席帰属」「活動状態」「内的状態」「キャラクター資材データ」のすべてが正常に揃っていること（AND条件）を前提として開始されます。どれか1つが欠けている状態で勝手に表示を開始してはなりません。また、排他性が確認できないクライアント上では表示を継続させません。画面に出力された特定の表情やモーションを、パートナーの精神状態のマスターデータや永続的な変化の証拠とみなしてはなりません。描画エンジンのクラッシュ、全画面表示による非表示、ローカルの高負荷といった表示側の問題を、テキスト対話、管理機能、システム復旧処理へ波及させてはなりません。

```rust
struct CheckBodyEligibilityQuery {
    companion: CompanionId,
    client: ClientId,
    presence: PresenceCheckRef,      // 現在の在席帰属の写し
    activity: CompanionActivityRef,  // 個体調整担当が管理する活動状態
}
// 判定結果は X-A や X-B の成立に従属する独立した巨大判定ではなく、
// 必要な前提条件がすべて満たされているかどうかの照合結果を返します。
```

### X-D Observation eligibility の連動

```rust
struct NotifyPresenceChangeFact {
    client: ClientId,
    present_companions: Vec<CompanionId>, // 正式な在席帰属に従ってその端末に存在するパートナー一覧（停止中を除く）
    fullscreen: FullscreenStateRef,  // 入出力・提示担当から通知される全画面表示状態
    observer_control: ObserverControlRef, // クライアント別またはシステム全体の観測一時停止／OFF設定
}

enum EligibilityDecision {
    EligibleForCapture,
    Ineligible(IneligibilityReasonRef), // パートナー全員が停止中、全画面表示中、観測OFF/一時停止中、アクティブな在席なし、費用上限超過など
}
```

- 画面キャプチャの対象やタイミングの判定は「共有観測担当」が、在席帰属の判定は「接続・存在担当」が行います。以前のクライアントで取得された古いキャプチャ画像や分析候補を使って、現在のクライアントに対する新しいキャプチャや情報配信を続けてはなりません。また、複数の対象クライアントに対して同時に画面キャプチャを実行してはなりません。

### X-E Observation result routing（三段階分離）

共有観測によるルーティング（Observer routing）と、各パートナー個別の推論処理（個体調整 ＋ 該当パートナーのプロバイダ設定）を、別の境界として厳格に維持します。

```rust
struct PublishObservationCandidate {
    client: ClientId,
    candidate: ObservationCandidateId,
    captured_at: WallClockWithTz,
    capture_range: CaptureRangeRef,  // デスクトップ画面全体（個別ウィンドウのみを対象としたと誤認させない）
    routing_context: RoutingContextOffer, // H-F で提供されたルーティング用の一時的な派生表現
}

enum RoutingDecision {
    RoutedTo(Vec<CompanionId>),      // 該当クライアント上で現在アクティブに動作している関連パートナーのみ
    SuppressedByControl,             // 一時停止／OFF設定、全画面表示、同意不足、費用超過、削除処理中など
    StaleCandidate,                  // 端末移動、停止、一時停止の前に取得された古い観測候補
}

struct DeliverEventNotification {
    candidate: ObservationCandidateId,
    target_companion: CompanionId,
    deliverable_range: DeliverableRangeRef, // そのパートナーが利用可能な観測内容と必要な背景情報
    // 通知を受信したことと、内容を理解・採用したことは別です。
    // Observer によるルーティングは、各パートナー自身の意味解釈、最終的な発話判断、
    // およびアクション実行の意思決定を勝手に代行するものではありません。
}

struct CompanionUnderstandingFact {
    companion: CompanionId,
    candidate: ObservationCandidateId,
    understood_as: UnderstandingRef, // 個別の文脈における意味判断（個体調整担当）
    experience_link: Option<ProposeExperienceCandidate>, // 自律学習の経験候補としての利用（H-B へ連携）
}
```

- プライベートな情報を不用意に全体公開したり、新しい自律学習のマスターデータに勝手に昇格させたり、全体共有（Global）にしたり、他パートナーへ無断で漏洩させてはなりません。要約やフォーマット変換によって元データの制約を勝手に消滅させてはならず、パートナー個別の設定を無視して同意を勝手に合成・代用してはなりません。複数のパートナー向けの情報が混ざり合って個別に分離できない場合は、配信を禁止します。また、あるパートナーが正常に理解・判断できたからといって、その結果を別のパートナーへ安易にコピーしてはなりません。配信後の個別パートナーによるプロバイダ送信は、独立した新たな利用として K-E および K-B の認可フローを改めて経る必要があります。

### X-F 移動・切断・再接続時の三者協調、X-G Host restart 時の協調

これらは X-A および X-B の確定事実や通知を適切に組み合わせることで実現されるため、不要な新規の巨大インターフェースを追加してはなりません。以下の基本原則を厳格に守ります。

- 在席帰属の切り替えは「接続・存在担当」、対話ラウンドの区切りは「入出力・提示担当」、クライアント依存アクションの区切りや成否不明の判定は「実行・拡張担当」、ホスト側で実行中の通常タスクの継続およびクライアント依存ステップの判断待ち・保留は「作業担当」、未伝達メッセージのホスト側保持および次回接続端末への要約報告は「個体調整担当」がそれぞれ確定します。端末移動の前提として、ホスト側で正常に継続可能なタスクを強制終了したり別の場所へ移送したりする必要はありません。
- 端末移動中やネットワーク切断中に遅れて届いたメッセージは、元の在席帰属、元のラウンド、元の試行記録に対応付け、用途ごとに厳格に受け入れを判定します。移動先の新しい活動へ勝手に付け替えたり、クライアント側のキャッシュコピーでホストのマスターデータを上書きしてはなりません。未送信のユーザー操作を勝手な自動アクションキューにしてはならず、遅延メッセージや表示用コピーによって、過去の在席、実行権限、タスク状態を勝手に復活させてはなりません。また、成否不明となった処理を勝手に未実行や成功へ改ざんしてはなりません。
- **ホスト再起動時**: 在席状態の復旧と、中断された対話ラウンド・Computer Use・タスク・アクションの再実行・再開は、まったく別の条件に基づきます。実行中（Running）だったパートナーは再起動前のクライアントへ安全に復帰を試みますが（元の端末が利用不能であれば在席なしとする）、中断されたタスクはオーナーによる明示的な再開指示を待ちます。ホスト停止中に到来したスケジュール実行はスキップ（Missed）扱いとし、未完了のデータ削除やバックアップ復元の安全保留は確実に維持します。

### X-H 未伝達の次 Client 報告

H-G による未伝達情報の登録に加え、新しいクライアント側でその要約を提示するためのインターフェースです。

```rust
struct RequestUndeliveredSummaryQuery {
    companion: CompanionId,
    to_client: ClientId,
    presence: PresenceCheckRef,      // 新しい在席が正式に成立した後の、現在の在席帰属
}

struct UndeliveredSummaryFact {
    items: Vec<UndeliveredRef>,      // 元のタスク記録や活動記録への対応関係付き
    current_filter: CurrentFilterRef, // 最新の処理結果、利用制限、データ削除状況との照合フィルター
}
// 要約の提示途中でクライアントが再び切断された場合は、提示状態を「成否不明」として保持します。
// 厳密な1回のみ配送（exactly-once）や、ユーザーが確実に読んだことの保証などを無理に追加しません。
```

## 7. Character クラスタの interface semantics（C-A〜C-D）

パッケージ検証（validation）の成功を、機能の実行許可やセキュリティ認可と混同してはなりません。

### C-A Character static revision の取得

```rust
struct GetCharacterRevisionQuery {
    character: CharacterId,
    revision: Option<CharacterRevision>, // None の場合は利用可能な全リビジョン一覧を取得
}

struct CharacterRevisionView {
    character: CharacterId,
    revision: CharacterRevision,
    parts: Vec<CharacterPartView>,       // 静的な設定部品、推奨スキルの指定（実行許可そのものではない）
    import_provenance: ImportProvenanceRef, // 取り込み時の由来情報（外部原本自体の所有権ではない）
}
// リビジョンが存在することと、それを適用すべきかどうかは別です。勝手な推測で適用してはなりません。
// データの提供は、確定済みの安全なリビジョンからのみ行います。
```

### C-B 既存 Companion への revision 適用

キャラクター資材のマスターデータ（Character担当）と、特定のパートナーへの適用関係のマスターデータ（個体調整担当）を、同一のトランザクションで混同して更新してはなりません。

```rust
struct ProposeCharacterApplicationCandidate {
    companion: CompanionId,
    character: CharacterId,
    expected_character_revision: CharacterRevision, // 期待されるキャラクターリビジョン（boundary token）
    selected_parts: Vec<CharacterPart>,  // オーナーが部品ごとに明示的に選択した更新範囲
    owner_selection: OwnerSelectionRef, // 管理画面での操作との対応関係
}

enum CharacterApplicationOutcome {
    AppliedAs(AppliedPartListRef),   // 個体調整担当が適用関係を確定（パートナーが積んできた経験や記憶は初期化しない）
    StaleRevision { current_applied: AppliedPartListRef },
    DeniedProhibitedPart(ProhibitedPartRef), // 認証情報、権限昇格、同意の代用など、適用が禁止されている危険な部品種別
    NeedsOwnerSelection,             // 部品ごとの明示的な選択が不足しているため確認が必要
}
// パッケージの更新によって、パートナーのこれまでの成長や思い出を勝手に初期化してはなりません。
// また、オーナーが確認していない部品を勝手に更新済みに変えてはなりません。
```

- 永続化は永続化グループA/B（`character_revision` ＋ `companion_applied_current` ＋ 変更履歴）で行い、コミットは同期区分 SD-CharApply の短いトランザクション内で不可分に完了します（CCT §4）。

### C-C Skill import・package validation

```rust
struct ImportSkillCommand {
    source: SkillImportSourceRef,    // パッケージ推奨スキルか単体インポートかの別 ＋ 外部参照情報
    scope_choice: SkillScopeChoice,  // パッケージ由来ならパートナー専用（Companion scope）を既定とし、単体ならオーナーが明示選択
    target_companion: Option<CompanionId>, // パートナー専用スコープの場合の紐付け先
}

struct ValidatePackageQuery {
    package_ref: ExternalPackageRef, // 外部パッケージの原本ファイルへの参照
}

struct PackageValidationReport {
    result: ValidationVerdict,       // 受け入れ可否の判定結果、含まれるコンテンツの範囲、信頼／非信頼の扱い
    import_provenance: ImportProvenanceRef,
    // 検証に成功したことは、認証情報の付与、実行権限の認可、全体共有（Global）化を意味しません。
}

enum SkillImportOutcome {
    ImportedAsCompanionSkill(SkillId),
    ImportedAsGlobalSkill(SkillId),  // 単体インポートにおいてオーナーが明示的に選択した場合のみ
    RejectedByScopeRule,             // 自動的な全体共有化や勝手な権限昇格に該当するため拒絶
    RejectedByValidation,
}
```

### C-D Character export

```rust
struct ExportCharacterCommand {
    character: CharacterId,
    revision: CharacterRevision,
    // 静的な設定・資材の範囲のみを選択して出力します。
    // パートナー固有の経験、自律学習データ、親密な関係性、内的状態、
    // 会話履歴、認証情報、権限設定を絶対に混入させてはなりません。
    // 出力内容と著作権等の権利上の注意書きを、画面上に明確に提示できるようにします。
}

struct CharacterExportFact {
    export_ref: ExportRef,           // 外部配布用のコピーファイル（内部所有権ではなく単なる成果物）
    included_scope: IncludedStaticScopeRef,
}
```

## 8. Data preservation / lifecycle クラスタの interface semantics（D-A〜D-E）

個人データ完全削除（Targeted Deletion）を、単なる汎用の `delete(entity_id)` のような安易な関数で表現してはなりません。また、調整役（coordinator）が各担当ドメインの内部状態を勝手に直接書き換えるようなインターフェースを作ってはなりません。

### D-A Targeted Deletion の開始・範囲確定

```rust
struct RequestTargetedDeletionCommand {
    purpose: DeletionPurposeRef,     // プライバシー保護やセキュリティ上の削除目的を明示（通常の忘却処理等との混同を防止）
    mechanical_condition: MechanicalConditionRef, // LLM に依存しない文字列一致などの厳密な機械的条件（必須レイヤー）
    semantic_hint: Option<SemanticHintRef>, // 意味的な補助ヒント（完全性の保証には使わない）
    exclusion: ExclusionRef,         // 削除から除外すべき対象や、事前確認が必要な範囲の対応関係
    impact_notice: ImpactNoticeNeedRef, // 削除の目的、対象範囲、影響度、除外対象に関するオーナー向け事前説明用データ
}

struct DeletionScopeDecision {
    operation: DeletionOperationId,
    sweep: DeletionSweepGeneration,  // 削除処理区間の厳密な順序世代
    valid_interval: ErasureInterval, // 削除処理の開始から検証完了までの有効期間（区間内に遅延到着したデータの取り込みを含む）
    participants: Vec<ParticipantOwnerRef>, // 消去処理に参加すべき全領域（オーナーに保存場所を手動選択させない）
}
```

- 保全・消去担当は、オーナーが指定した明示的な目的、対象、影響範囲、除外項目を正しく紐付け、消去に参加すべき内部領域の特定を確実に支援します。特定文字列の機械的な検索、データ消去、残存検証の成否を、LLMの推論結果に依存させてはなりません。

### D-B 参加者への局所消去要求・局所完了返却

```rust
struct DemandLocalErasureCommand {
    operation: DeletionOperationId,
    sweep: DeletionSweepGeneration,
    erasure_condition: ErasureConditionRef, // 各受入・保存先が照合すべき消去条件、有効期間、完了境界
}

struct ParticipantCompletionFact {
    operation: DeletionOperationId,
    participant: ParticipantOwnerRef, // データの担当責任者だけでなく、処理中コンテキスト、派生データ、キャッシュ、クライアントや外部拡張の一時コピー、返却待ち結果などを保持するすべてのデータ保持者
    local_result: LocalErasureResult, // 処理成功／検証完了／未完了／失敗／未確認範囲の各状態
    source_relation: SourceRelationRef, // 元データとの関係、処理中の利用状況、局所的な消去・検証状況、再保存の防止措置、未確認範囲の対応関係
    // 一部の局所的な消去が完了したことと、システム全体の削除完了は別です。
    // 未確認の領域や到達不能な端末を「成功」とみなしてはならず、局所的な完了報告だけで安全保留（hold）を解除してはなりません。
}
```

- 各データ保持者は、`(元データとの関係, 処理中の利用状況, 局所消去と検証, 再保存防止, 未確認・未完了範囲)` の状態を保全・消去担当へ正確に報告します。元データを消去した後に依存関係まで一緒に失われ、遅延して届いた結果が削除対象かどうか識別できなくなるような実装は禁止します。対象の本文そのものは保持せずとも、識別子やハッシュ値によって対応関係を確実に辿れなければなりません。

### D-C 残存検証・全域完了確定

```rust
struct VerifyRemainderQuery {
    operation: DeletionOperationId,
    sweep: DeletionSweepGeneration,
}

enum RemainderVerification {
    NoRemainderMechanically,         // 機械的な残存検証の成功（意味論的な100%完全性の保証ではない）
    RemainderFound(RemainderRef),
    UnreachableScope(UnreachableRef), // オフライン等の到達不能領域が存在する場合は「成功」にしない
}

enum GlobalDeletionCompletion {
    GloballyCompleted,               // システム全域での削除完了（保全・消去担当が確定。各ドメインの意味論的な変更は各担当者が行う）
    HeldPending(PendingRef),         // 未完了の参加者がいるため安全保留および再保存防止を維持
    FailedVerification,              // 検証に失敗
}
// 全域での削除完了とは、システム内部の全域におけるデータの除去または復元不能化、
// 機械的な残存検証の成功、処理区間内に遅延到着したデータの取り込み、再保存・再学習の防止、
// およびすべての未完了状態の集約が完全に満たされた状態を指します。
// 完了記録や監査ログに対象の本文データを書き戻してはなりません。
// 完了時には機械的条件の検索用トークンを完全に消去（wipe）し、完了後のバックアップにトークンを残してはなりません。
```

### D-D Backup 作成・Restore staging・マスターデータ切り替え・復元後保留・一括有効化

```rust
struct CreateBackupCommand {
    setting: BackupSettingRef,       // 保存先、個別スケジュール、保持世代数、保護レベルの設定
}

struct BackupPointFact {
    backup_point: BackupPointId,
    covered_scope: CoveredScopeRef,  // 対象時点、参照の整合性、除外されたデータ（秘密値や外部実体）、処理中タスクの未完了状況
    // 各テーブルのコピーが個別に成功したことだけでバックアップ成功とみなしてはなりません。
    // 対象時点、参照関係、変更履歴、未完了状況の整合性が完全に揃って初めて成功と確定します。
}

struct RequestRestoreCommand {
    backup_point: BackupPointId,
    // 事前説明事項：復元対象、バージョン互換性、外部ファイルが巻き戻らないこと、
    // 削除済みデータや過去のルール・同意・スケジュールの復活可能性、再認証が必要となること。
}

struct StagedRestoreCandidate {
    restore: RestoreId,
    backup_point: BackupPointId,
    isolation_check: IsolationCheckRef, // ステージング領域（別ファイルまたは別グループ）での整合性照合と検証
}

struct SwitchRestoreDecision {
    restore: RestoreId,
    new_generation: RestoreGeneration, // 復元成功ごとに単調増加する世代番号（過去の実行コンテキストを安全に分離）
    hold: HoldConditionRef,          // 復元後の一括安全保留
    // 切り替え前は「復元前の正常状態」がマスターデータであり、
    // 切り替え後は「復元されたデータ」がマスターデータとなります。両者が混ざり合った第三の状態を作ってはなりません。
    // マスターデータの切り替えの瞬間のみ、同期区分 SD-Restore による短い排他制御を行います（長期間の読み取り停止は行いません）。
}

struct BulkEnableAfterRestoreCommand {
    restore: RestoreId,
    // オーナーが復元内容を確認した後、安全にまとめて有効化できます（1件ずつの手動再承認は不要）。
    // ただし、復元された割り当て設定や同意情報だけで、外部処理を自動再開してはなりません。
    // 現在の認証情報、現在の制約、復元後保留、拒絶設定、費用上限、認証不足、成否不明の状態を無視してはなりません。
}
```

- バックアップからの復元（Restore）は、バックアップ開始前の認証情報の秘密値を除き、対象となる内部データをすべて置き換える処理であり、既存データとの「マージ」ではありません。復元に失敗した場合は、復元前の正常な状態をそのまま維持します。過去の実行結果やクライアント側のキャッシュコピーを、新しいマスターデータへ混入させてはなりません。外部ファイルの現在の内容や存在、過去に実行された外部作用、認証成功の事実は巻き戻りません。復元された参照情報を現在の保管庫と照合し、不足や無効があればオーナーへ再認証を要求します。

### D-E 通常保持・retention / cleanup・Reset

データの削除や初期化の目的を、単一の処理にひとまとめにしてはなりません。通常の忘却、訂正、統合、失効、置換、通常の会話履歴・ログの定期削除、容量制限に基づく削除（既定はOFFで明示的なオプトインが必要）、パートナーの個別削除、個人データ完全削除、設定の初期化、全データの完全初期化、バックアップ復元は、それぞれまったく別のライフサイクルと完了条件を持つ処理として厳格に扱います。

```rust
struct ApplyRetentionCommand {
    policy: RetentionPolicyRef,      // 通常の履歴・ログ保持ポリシーか、容量保持ポリシー（既定OFF）かの別を明示
    target_range: RetentionTargetRef, // 指定日以前などの消去対象範囲
}

struct ResetCommand {
    kind: ResetKind,                 // 設定のみ初期化（Settings）か、全データ初期化（FullData）かの種別
    enumeration: ResetEnumerationRef, // 削除対象となるデータの明示的な列挙と、確実な事前確認（全データ初期化時）
}
// 設定の初期化（Settings Reset）は一般設定を既定値に戻す処理であり、
// パートナー個体、対話履歴、要約、自律学習データ、親密な関係性、活動状態、
// タスク、スケジュール、認証情報、ルール、同意、費用上限を削除してはなりません。
// 全データ初期化（FullData Reset）は、ホスト内部のすべてのデータと認証情報を安全に削除しますが、
// 外部ワークスペース、ワークスペース内のスキルファイル、オーナーが別途退避させたバックアップファイルは削除しません。
```

## 9. 管理経路の独立（第一者管理面 → 各 owner）

公式（第一者）の入出力・提示担当は、作業担当へのタスク中断・スケジュール管理・記録確認、個体調整担当へのパートナー停止・削除、権限・制約担当への承認・拒絶・ルール・同意・費用枠・デバイス管理、認証情報管理への明示的な設定、保全・消去担当へのデータ削除・バックアップ・復元・初期化を、直接要求できます。ここで「直接」とは、パートナー自身の思考LLMによる承認や、長時間タスクの完了を待つ必要がないという意味であり、特定のAPI形式を強制するものではありません。各担当責任者による要求の受理・前提確認・結果判定に従う必要があり、UI層にシステムの状態を勝手に書き換える特権を与えるわけではありません。要求の受付と完了、保存済みデータへの影響、判明している作用と不明な状態を、画面上で明確に区別して提示します（H-10 / X-10 / K-1 / D-A〜D-E に共通する管理インターフェースの性質）。

特に権限の大きい重大な操作の最終確認は、ホストPC本体の信頼できる公式管理画面（trusted first-party management surface）に厳格に限定します（[要件「信頼境界」](../../requirements/requirements.md#信頼境界)、具体的な対象と確認境界は IPC §18 を参照）。ペアリングの承認やデバイスの信頼・失効、認証情報の変更、バックアップ復元および復元後の一括有効化、全データ初期化などの要求は、リモート端末から送信することは可能ですが、リモート端末側で「確認した」と自己申告したことだけを根拠にして処理を実行してはなりません。担当者は、ホスト本体の画面で確認された操作の由来と、対象データおよび現在の前提条件に結び付く厳格な確認事実を必要とします。コマンドのデータが存在していることだけで「確認済み」と誤認してはなりません。ホストと同居する通信経路やペアリングが存在することだけで無条件の信頼を与えてはならず、ツールや Computer Use 機能などの代理入力による最終確認も一切受け入れません。なお、管理機能全体に対して、会話がアクティブでなければならないという制約は課しません。

```rust
struct ManagementOperationCommand {
    operation: ManagementOperationKind, // StopCompanion | DeleteCompanion | CancelTask |
                                       // ManageSchedule | DenyOrRefuse | ManageRuleConsentCap |
                                       // ManageDevice | ConfigureCredential | RequestDeletionBackupRestoreReset
    target: ManagementTargetRef,
    // パートナー自身の思考LLMによる事前承認や、長時間タスクの完了を介在させてはならない。
    // LLM の応答成功を管理操作の必須条件にしてはならない。
}
```

## 10. Boundary token の具体的な渡し方（field matrix）

何でも詰め込んだ巨大な共通コンテキスト構造体を安易に引き回してはなりません。各境界で必要とされる前提条件のみを、厳格に型付けして受け渡します。トークンは決定権威そのものではなく、現在の最新状態と比較照合するための材料であり、値が不一致の場合は新しい処理を開始せず、安全保留、拒絶、情報不足、再評価へ差し戻します。前提トークンが省略された場合は「制約がない」のではなく「前提不明による不受理」とします。

**凡例**: ● ＝ 必須フィールド、○ ＝ 該当する場合に必須、— ＝ 当該境界では保持しない。※「世代」は在席世代（presence）、復元世代（restore）、消去スイープ世代（sweep）を型として明確に区別し、決して混同しません。

| インターフェース境界 | 期待される識別子＋リビジョン | 期待される世代番号（型付き） | 発生元／消費主体 | 目的・利用用途・スコープ | 操作種別／試行ID | 結果の確信度 | 安全保留／削除／復元との関係 |
|---|---|---|---|---|---|---|---|
| H-A タスク化／方針指示／委任 | ● `TaskRef` ＋ ● `task.progress` 非 terminal（委任 AU3・方針指示 AU4。terminal は `TaskTerminal` で書き込みなし） | ○ 復元前提（`RestoreGeneration`。復元跨ぎ参照防止。保全・消去スライスで導入） | ● 委任元パートナー・タスク・ワークスペース | ● 作成時の目的・方針指示前提（`SteeringPremiseRef`）・委任スコープ | ○ `DelegationRef` | —（達成は作業担当、確信度は実行担当） | ● 安全停止・保留・削除・復元保留の照合（hold スライスで導入） |
| H-A エージェント結果受入 | ● 委任対応（delegation 行）から解決する依拠 `TaskRef` と、現在 `task.revision` の一致 | ○ 復元／消去の各世代タグ（producer スライスで追加） | ● 結果 identity・委任（execution lifetime = 1 delegated Task Agent execution）・依拠 revision。final result の到着で委任を seal | ● 現在の目的 identity との整合性（本文文字列の一致は使わない） | ● 依拠 Action 試行相関（delegation から列挙した authoritative set と claim の完全一致。欠如・追加・重複は技術的エラー。membership は seal 時点で固定）。確定度は owner 事実のまま（完了は result-local の非成功と、同じ `TaskId` の全 revision / 全 delegation に残る `Unknown` の集合が空のときにのみ成立。Task-wide completion barrier） | ● 結果本文は Task owner が到着時に 1 行だけ保存（自己申告を確信度にしない） | ● 中断（cancel marker は後続 producer）・方針変更・terminal・execution seal・削除・復元との照合 |
| H-B 経験提出 | ○ タスク由来なら `TaskRef` | ○ 在席／復元／消去の各世代タグ | ● 経験したパートナー・タスク・委任 | ● 想定用途・期待スコープ | — | — | ● 保存禁止・非共有・消去条件の照合 |
| H-C 会話による訂正 | ● `(LearningId, expected_revision)` | ○ 消去スイープタグ | ● 訂正対象・根拠となる新経験 | ● 時間的な意味論（最初から誤り／変化） | — | — | ● 消去・制約条件の照合 |
| H-D 自律学習参照 | ○ `(LearningId, revision)`（取得データ側） | ○ 消去スイープタグ | ● 利用元・目的・要求スコープ | ● 用途・スコープ・制約の写し | — | — | ● スコープ制限・保存禁止・消去保留 |
| H-E スコープ変更 | ● `(LearningId, expected_revision)` | ○ 消去スイープタグ | ● 対象項目・共有内容 | ● 共有内容・背景・非共有意図 | — | — | ● オーナーの明示的制約の照合 |
| H-F ルーティング文脈供給 | ○ 元データとの対応 | ○ 在席／消去の世代タグ | ● 元の担当責任者・対象パートナー・用途 | ● ルーティング用途・制約・選択前提 | ○ 観測候補との対応 | — | ● 利用同意・消去・失効状態の照合 |
| H-G 未伝達メッセージ管理 | ○ 元データとの対応（Task／Activity） | ● 在席世代 ＋ 復元世代 | ● 対象パートナー | ● 提示・報告の進行状況 | ○ 対話ラウンドID | ○ 提示完了／成否不明 | ● データ削除状況の照合 |
| K-B リアルタイム認可照合 | ● タスク／ルール／同意の期待リビジョン | ○ 在席／復元／消去の各世代タグ | ● 実行主体・委任・タスク・ワークスペース | ● 目的・対象・操作・データ・送信先・費用 | ○ 試行前提 | — | ● 認証失効・安全停止・費用上限・保留 |
| K-C 秘密情報利用 | ○ `CredentialRef`（非秘密）＋ 用途 | — | ● 接続先・用途 | ● 認証用途の厳格な制限 | ○ 操作種別 | ○ 認証成否・再認証要否 | ● 認証失効・安全保留の照合 |
| K-D/E/F 推論実行・フォールバック | ○ 同意情報の期待リビジョン | ○ 復元／消去の世代タグ | ● 消費主体・要求能力 | ● 用途・データ・送信先・取扱制限・費用 | ○ チケットID | ○ 能力不足・送信失敗・実測利用量 | ● 保存禁止・費用上限・保留・消去 |
| K-G 利用枠予約・確定 | ○ `CapId` ＋ 帰属先 | — | ● 消費主体・用途・送信先 | ● 利用枠上限・引当予約量 | ○ 予約ID | ○ 確定実測値／成否不明／処理中 | ● 上限超過・費用不明の照合 |
| K-H アクション実行開始 | ● タスクリビジョン・ルール／同意の前提 ＋ ● `task.progress` 非 terminal（terminal は `TaskTerminal` で開始なし） ＋ ● execution 非 seal（seal 済み delegation は `ExecutionSealed` で開始なし） | ● 在席 ＋ 復元 ＋ 消去（該当するもの） | ● 実行主体・委任・タスク・ワークスペース（委任 = execution lifetime。Action attempt は delegation で結果の authoritative set に束ねられる） | ● 目的・実対象・操作・データ・送信先・費用 | ● 認可判定ID ＋ 費用予約ID | —（開始時点） | ● 認証失効・安全停止・費用上限・保留・消去・復元 |
| K-H 外部結果確定 | ● 試行ID ＋ 期待される確信度 | ○ 世代タグ | ● タスク・委任 | ● 対象・外部作用との対応関係 | ● 元となった成否不明試行ID | ● 確認済み成功／確認済み失敗／成否不明 | ● 中断・再実行条件の照合 |
| X-A 在席移動 | ● `(CompanionId, expected_generation+state)` | ● `PresenceGeneration` | ● 対象パートナー・移動元端末・移動先端末 | ● 移動の発生理由 | ○ ラウンド終了対応 | ○ 到達性・排他性の確認 | ● ペアリング・デバイス・安全停止・保留 |
| X-B 対話ラウンド受付 | ○ `(Companion, Client, round)` ＋ 申告世代 | ● `PresenceGeneration` | ● 対象パートナー・端末・ラウンド・観測候補 | ● 対話の目的 | ○ ラウンドID・試行ID | ○ 受付完了／提示完了／成否不明 | ● 端末切り替え・安全停止・保留・消去 |
| X-E 観測結果ルーティング | ○ 候補ID ＋ 文脈との対応 | ○ 在席／消去の世代タグ | ● 取得元端末・対象パートナー・用途 | ● ルーティング用途・配信制約 | ○ 観測候補ID | — | ● 観測制御・利用同意・消去の照合 |
| D-B 局所データ消去 | ● `(operation, sweep)` ＋ 元データとの対応 | ● `DeletionSweepGeneration` | ● 参加担当者・データ保持の対応関係 | ● 削除目的・対象範囲・除外設定 | — | ○ 局所的な検証結果 | ● 有効期間・完了境界・安全保留 |
| D-D バックアップ復元 | ● バックアップID ＋ 世代前提 | ● `RestoreGeneration` | ● 復元対象・参照関係の対応 | ● 用途・再有効化の範囲 | ○ 復元ID／ステージングID | ○ ステージング検証・マスター切り替え成否 | ● 復元後の一括保留・認証情報の照合 |
| C-B キャラクター適用 | ● `(CharacterId, expected_revision)` ＋ 適用ポインタ | ○ 復元／消去の世代タグ | ● 対象パートナー | ● 選択部品・オーナーの明示選択 | — | — | ● 適用禁止部品種別の照合 |

`PresenceGeneration`（在席世代）、`RestoreGeneration`（復元世代）、`DeletionSweepGeneration`（消去スイープ世代）は、それぞれ独立した別の型・別のフィールドであり、絶対に混同してはなりません。また、タスクの方針変更は `TaskRevision` を進めることで表現し、世代番号の変更で代替してはなりません（CI §6.4）。

## 11. Acceptance result と error model（domain outcome と technical error の分離）

単純な `Result<T, Error>` だけでは、期限切れ（stale）、権限拒絶（denied）、安全保留（held）、後続処理による置換（superseded）、権能不足（insufficient）、外部成否不明（unknown-external）、履歴記録のみ（historical-only）といったドメイン上の多様な状態を表現しきれません。かといって、全ドメイン共通の巨大なステータスコード enum を作ってはなりません。システム障害などの技術的例外（technical failure）と、業務判断による受領・拒絶・保留（semantic outcome）を厳格に分離します。

### 11.1 返却形の約束

```rust
// ドメインの判定結果は Ok 側、技術的な障害は Err 側として型定義する。
type InterfaceResult<Outcome, TechErr> = Result<Outcome, TechErr>;

// 例：タスク提案インターフェース
fn propose_task(cmd: ProposeTaskCommand)
    -> impl Future<Output = InterfaceResult<TaskProposalOutcome, TaskTechnicalError>>;

// 例：アクション実行要求インターフェース
fn request_action(cmd: ExecuteActionCommand)
    -> impl Future<Output = InterfaceResult<ActionStartOutcome, ActionTechnicalError>>;
```

- `Ok(...)`: 担当責任者が確定したドメイン上の受入判定結果（正常受理、期限切れ、権限拒絶、安全保留、情報不足、再評価待ち、過去記録への保存のみ等）。呼び出し側は受け取ったバリアントに従って、安全保留、再評価、オーナーの指示待ちへ正しく遷移させます。勝手にキューに溜めたり裏で再試行したりしてはなりません。
- `Err(...)`: データベースの接続不能、プロバイダの通信障害、OSのI/Oエラーなどの技術的障害。業務上の採否判定ではありません。技術的エラー発生時の再試行の可否は、並行性制御（CCT）およびアクション実行（AE）の成否不明・重複防止契約に従います（自動的な再実行は行いません）。

### 11.2 domain outcome enum の一覧（domain ごとに区別）

共通の巨大 enum に安易に統合してはなりません。各 enum は、該当する担当責任者のドメインにおける固有の意味論のみを持ちます。

| ドメイン領域 | 結果判定 enum（例） | 主なバリアントの意味論 |
|---|---|---|
| タスク提案・方針指示・委任（コマンドレベル） | `TaskProposalOutcome`、`DelegationOutcome` | 受理（Accepted）／前提不一致（StalePremise：現在値付き）／タスク終端（TaskTerminal：Completed / Failed のため書き込みなし。方針指示・委任時）／タスク未存在（MissingTask：方針指示・委任時）／リビジョン上限超過（RevisionExhausted：方針指示時のみ）／全体保留中（HeldByGlobalHold：hold スライスで追加）／再照合が必要（NeedsRevalidation）／情報不足（InsufficientContext）。TaskProposalOutcome は候補側の意味論をすべて持ち、DelegationOutcome は HeldByGlobalHold・NeedsRevalidation・InsufficientContext を、それぞれの producer を持つスライスで追加します |
| タスクコミット（方針指示 AU4、リポジトリレベル） | `TaskCommitOutcome` | コミット成功（CommittedAs）／期待値不一致（StaleExpected：現在値付き）／タスク終端（TaskTerminal：Completed / Failed のため書き込みなし）／タスク未存在（MissingTask）／リビジョン上限超過（RevisionExhausted）／全体保留中（HeldByGlobalHold：hold スライスで追加） |
| エージェント結果受入 | `TaskResultAcceptance` | 採用と同時に完了（AdoptedAsCompletion：到着 record 済み・現在リビジョンと一致・delegation（execution lifetime）から列挙した authoritative set が claim と完全一致・依拠試行がすべて ConfirmedSuccess・同じ TaskId の全 revision / 全 delegation に Unknown が無い（Task-wide completion barrier））／元の依拠リビジョンへ記録のみ（RecordedToOriginalOnly：revision 前進・terminal・cancel marker）／完了を保留（WithheldByEffectFacts：blockers = authoritative set に Unknown / ConfirmedFailure ∪ 同じ TaskId の Task-wide barrier に残る Unknown。重複は 1 回。到着 record と result-local 相関は残り現在 Task は不変。seal 後の証拠更新で再評価可能）／採用対象の result 行が無い（MissingResult）／委任・タスクの durable 状態が無く書き込みなし（MissingDelegation / MissingTask。識別子不在は技術的エラーにしない）。同じ TaskId の別 delegation / 旧 revision の試行は barrier のためにのみ読まれ、`task_result_attempt` には刻印しない。同一 delegation への 2 つ目の final result 到着は AU15a の durable invariant 違反として fail closed（domain outcome ではない） |
| 経験提出・訂正・スコープ | `FormationDecision`、`CorrectionOutcome`、`ScopeDecision` | 知識形成（Formed）／保留（Deferred）／保存価値なし（Declined）／訂正完了（Corrected）／パートナー専用を維持（KeptAsCompanion）／明示制約により拒絶（DeniedByExplicitConstraint）／対象期限切れ（StaleTarget）／消去中保留（HeldByErasure） |
| 権限リアルタイム照合 | `LiveAuthorizationDecision` | 今回の利用を認可（AllowForThisUse）／拒絶（Deny）／オーナー確認待ち（AskOwner）／条件充足待ち（WaitForCondition）／再照合が必要（NeedsRevalidation） |
| 認証秘密利用 | `AuthenticatedUseOutcome` | 規定範囲で安全に利用（UsedWithinScope）／再認証が必要（NeedsReauthentication）／制約により拒絶（DeniedByConstraint）／参照期限切れ（StaleReference） |
| 推論実行・フォールバック | `InferenceDispatchOutcome`、`FallbackDecision` | 完了（Completed）／送信前拒絶（NotSent）／承認済みフォールバックとして許可（AllowedAsApprovedFallback）／未承認経路のため拒絶（DeniedAsUnapprovedRoute） |
| 利用枠予約・確定・解放 | `ReservationOutcome` | 予約成功（Reserved）／上限超過で拒絶（DeniedByCap）／費用不明のため保留（HeldForUnknownCost）／再照合が必要（NeedsRevalidation） |
| アクション開始・確定 | `ActionStartOutcome`、`LateArrivalHandling` | 試行開始（StartedAsAttempt）／拒絶（Denied）／オーナー指示待ち（AskOwner）／前提不一致（StalePremise）／タスク終端（TaskTerminal：Completed / Failed のため開始なし。Action 側は Task lifecycle 語彙を import しない）／実行 seal 済み（ExecutionSealed：delegated execution が final result で seal 済みのため開始なし。Task lifecycle 語彙を import しない）／全体保留中（HeldByGlobalHold）／元記録へ保存（RecordedToOriginal）／成否不明のまま重複リスク提示（KeptUnknownWithDupRisk）／消去条件により再保存抑止（SuppressedByErasure） |
| 在席移動・対話ラウンド | `MoveDecision`、`RoundIntakeOutcome` | 新端末へ移行開始（TransitioningToNew）／古い在席情報のため拒絶（RejectedAsStalePresence）／制約により拒絶（DeniedByConstraint）／ラウンド受理（AcceptedForRound）／過去ラウンドのため拒絶（StaleRound）／移行中保留（HeldForTransition） |
| 観測結果ルーティング | `RoutingDecision` | 指定パートナーへ配信（RoutedTo）／制御設定により抑止（SuppressedByControl）／古い観測候補のため破棄（StaleCandidate） |
| キャラクター適用・インポート | `CharacterApplicationOutcome`、`SkillImportOutcome` | 適用完了（AppliedAs）／リビジョン不一致（StaleRevision）／禁止部品のため拒絶（DeniedProhibitedPart）／オーナー選択待ち（NeedsOwnerSelection）／スコープ別インポート完了／スコープ規則違反で拒絶（RejectedByScopeRule）／検証失敗で拒絶（RejectedByValidation） |
| データ削除・検証・完了 | `ParticipantCompletionFact`、`RemainderVerification`、`GlobalDeletionCompletion` | 各参加者の局所処理・検証・未完了・失敗・未確認状態／機械的残存なし（NoRemainderMechanically）／残存データ検出（RemainderFound）／到達不能領域あり（UnreachableScope）／システム全域完了（GloballyCompleted）／未完了のため保留維持（HeldPending）／検証失敗（FailedVerification） |
| バックアップ・復元 | `BackupPointFact`、`SwitchRestoreDecision` 等 | 対象時点、参照整合性、変更履歴、未完了状況がすべて揃って初めて成功と確定（個別テーブルのコピー成功だけを全体の成功とみなさない） |
| リポジトリ比較コミット | 各種 `*Outcome`（第13節） | コミット成功（CommittedAs）／期待値不一致（StaleExpected）／タスク未存在（MissingTask）／リビジョン上限超過（RevisionExhausted）／全体保留中（HeldByGlobalHold：hold スライスで追加。ドメインごとの具象バリアントを指し、`HeldByOperation` という共通 enum は設けない） |

「過去の証拠としてのみ受け入れる（accepted only as historical evidence）」状態は、`RecordedToOriginalOnly`（Task 結果受入）や K-K の `RecordedToOriginal` といった明示的なバリアントで表現します。「記録としては受け入れるが、現在の活動には採用しない」という判定は、元のアクションやタスクの過去リビジョンへの保存と、現在のタスクや認識への反映を明確に切り離すことで表現します（H-A、K-H、K-K）。

### 11.3 technical error の配置（`thiserror` 等）

- 技術的エラー型は、各インフラストラクチャ層や担当ドメイン層で明示的に定義します。ライブラリの公開エラーとして、型のない生の `String` や `Box<dyn Error>` を使用してはなりません（リポジトリ共通規約）。
- エラー型の定義例（命名の分かりやすい調整は許容）：
  - `TaskTechnicalError`（作業担当の永続化層）：`StorageUnavailable`（ストレージ利用不可）、`CommitConflictIo`（比較照合自体のI/Oエラー。前提値の不一致とは別）など。
  - `ActionTechnicalError`（実行・拡張担当のアクション層）：`DeviceIoFailed`（デバイスI/Oエラー）、`SandboxLaunchFailed`（サンドボックス起動失敗）など。権限拒絶、前提不一致、安全保留は含めません。
  - `InferenceTechnicalError`（推論担当の通信層）：`ProviderTransportFailed`（プロバイダ通信失敗）、`ResponseLost`（応答の消失。成否不明の要因であり、確信度の勝手な改ざんは行わない）など。
  - `PresenceTechnicalError`（接続・存在担当の観測層）：`ReachabilityCheckFailed`（疎通確認自体の失敗。パートナーが不在であると断定したわけではない）など。
  - `PreservationTechnicalError`（保全・消去担当のストレージ層）：`BackupIoFailed`（バックアップI/O失敗）、`StagingCorrupted`（ステージング領域の破損）など。検証失敗や端末到達不能とは厳格に区別します。
- 業務上の判定結果（domain outcome）と技術的例外（technical error）を同一のエラーカテゴリに混同してはなりません。期限切れ（`stale`）、権限拒絶（`denied`）、安全保留（`held`）、現在無効（`not-current`）、識別子不在（`missing`）、リビジョン枯渇（`revision exhausted`）、上限超過（`cap exceeded`）などは、すべて正常系（`Ok` 側）のバリアントであり、ネットワークエラーのような自動再試行（retry）の対象ではありません。呼び出し側が通信エラーを「権限拒絶」と誤読し、ユーザーへ誤った画面表示を行わないように徹底します（CC-07）。
- 識別子不在（前提が指す識別子の永続化状態が存在しない）やリビジョン枯渇はドメイン判定結果（domain outcome）であり、`*_TechnicalError` として扱ってはなりません。逆に、部分的・不整合な永続化単位（partial / inconsistent な durable unit）は技術的エラーであり、ドメイン判定結果として読み替えてはなりません。この切り分けは、各担当責任者のコミット・読み取り境界において同一の規則として厳格に遵守します。

## 12. Long-running operations — request と completion の分離

LLMによる推論、タスクエージェントの自律処理、外部ツールの実行、Computer Use、バックアップ作成および復元、個人データ完全削除などの長時間処理について、処理の開始要求インターフェース（request）と、結果の到着・完了報告インターフェース（completion / result）を明確に分離します。要求オブジェクトをプロセスメモリの中だけで保持していないと結果の帰属が分からなくなるような、脆弱な設計を徹底して排除します。

| 長時間処理の種類 | 開始要求（request） | 結果到着・完了報告（completion / result） | 永続化により復元可能な対応関係（durable correlation） |
|---|---|---|---|
| 推論実行（単発・継続・フォールバック・再送） | `AdmissionRequest` / `AuthorizedInference` → 試行の確定（チケット発行、費用予約、前提条件の確定） | `InferenceResultArrival`（チケットIDから結果テキストおよび利用量への対応付け） | `(チケットID, 消費主体, タスク／委任との対応, 利用目的, リビジョン／世代前提, 由来情報)`。PR グループF/I、CI §6.4 の世代タグ。実装では推論試行行（`inference_attempt`）が ticket から消費主体・利用目的・委任・依拠 `TaskRef` を辿れるように保持し、利用実績（`usage_fact`）からの帰属は ticket 対応で解決します。結果到着・採用・確定度は試行行に持たせず、それぞれの producer が追加します。再起動時の読み出しは ticket を境界にした bounded read（`load_inference_attempt(ticket)`）で行い、provider の自動 replay やエージェントの自動再起動の根拠にしてはなりません。 |
| タスク委任・自律エージェント | `CreateDelegationCommand`（期待リビジョン ＋ `task.progress` 非 terminal の不可分な比較照合。AU3 の作成スライスは委任 ID・依拠 TaskRef・委任元・ephemeral ID・スコープの写しを永続化し、progress を `Started → InProgress` へ進める。委任 = 1 delegated Task Agent execution の lifetime） | `TaskAgentResultArrival`（finalization 境界で到着 record を先に durable 化し、同じ不分区間で delegation を seal。その後に採用判定。結果本文の identity と 1 回だけの保存、claim の authoritative set 検証、Task-wide completion barrier の検証、採用と完了確定） | `(結果ID, 委任ID（execution lifetime）, 依拠 TaskRef, 結果本文 1 行, 依拠 Action 試行相関（authoritative set。seal 時点で固定。result-local）, 採用リビジョン, Task progress)`。完了は result-local 相関の検証と Task-wide unresolved Action completion barrier（同じ TaskId の全 revision / 全 delegation に Unknown が無いこと）の両方で確定する。依拠リビジョンは delegation 行、目的はそのリビジョンの `task_revision` snapshot から解決。execution lifetime は 1 delegation（0..N inference turn / 0..N Action attempt / 0..1 final result）であり、final result 到着で seal する。Action 試行の確定度は実行・拡張 owner の事実のまま参照のみ。再起動後は `(result_id → task_result → delegation → task_revision → task_result_attempt)` の bounded read（`load_task_result`）で辿り、到着済み・採用未確定の結果は本文を保持したまま提示できる。PR グループD（Task）/E（Action）、CI §5.3 |
| アクション試行・外部ツール・Computer Use | `ExecuteActionCommand`（実行前の不可分な比較照合。`task.progress` 非 terminal と execution 非 seal を含む）→ `StartedAsAttempt(attempt)` | `ReportEffectFact`（試行ごとの CAS 更新）＋ `LateArrivalAttribution`（遅延到着の帰属） | `(試行ID, 委任（execution lifetime）, タスクリビジョン前提, 実際の操作対象と操作種別, 依拠した認可ID, 在席／復元世代, 元の成否不明試行ID)`。PR グループE |
| バックアップ作成 | `CreateBackupCommand` | `BackupPointFact`（対象時点、参照関係、未完了状況の整合性が揃って確定） | `(バックアップID, 対象時点・参照整合性・除外データ・未完了状況)`。PR グループJ |
| バックアップ復元 | `RequestRestoreCommand` → `StagedRestoreCandidate`（隔離ステージング検証） | `SwitchRestoreDecision`（復元世代の更新 ＋ マスター切り替え）→ `BulkEnableAfterRestoreCommand` | `(復元ID, バックアップID, RestoreGeneration, 安全保留, 認証情報照合)`。PR グループJ |
| 個人データ完全削除 | `RequestTargetedDeletionCommand` → `DeletionScopeDecision` | `DemandLocalErasureCommand` → `ParticipantCompletionFact` → `VerifyRemainderQuery` → `GlobalDeletionCompletion` | `(操作ID, 消去スイープ世代, 有効期間, 参加者対応, 安全保留)`。PR グループJ、CI §5.9 |
| 未伝達メッセージ報告 | `RegisterUndeliveredFact`（親の処理と不可分に登録） | `RequestUndeliveredSummaryQuery` → `UndeliveredSummaryFact` → `PresentationMark`（画面提示確認後にのみ確定） | `(未伝達ID, 元データとの対応, 報告状況, ラウンド・世代対応)`。PR グループB |
| キャラクター適用 | `GetCharacterRevisionQuery`（資材供給） | `ProposeCharacterApplicationCandidate`（オーナー選択を伴う提案 → 適用確定） | `(キャラクターID, リビジョン, 選択部品, オーナー選択情報)`。PR グループA/B |

完了報告を受け取る側は、永続化された情報から元の識別子、リビジョン、世代、試行番号、操作種別、発生由来を確実に辿れます。遅れて届いた成果物は、`試行ID → タスクリビジョン → 現在のタスク` の順序で厳格に照合し、「過去の事実としての記録」と、「現在の活動への採用・画面提示・後続処理の自動開始」を明確に切り離します（CI §6.3）。中断、失効、方針変更、端末移動、データ削除、バックアップ復元の後に遅れて到着した結果は、元の古いアクションやタスクの過去ログへ正しく記録するに留め、過去の古い承認を勝手に復活させたり、古い結果を新しい目的に勝手に流用したり、後続の処理を自動開始してはなりません。

## 13. Repository interfaces — compare-and-commit を成立させる形

単純な「読み込み（load）→ メモリ上で編集 → 保存（save）」という手続きだけでは競合（race condition）が発生してしまう各ドメインにおいて、期待リビジョン／世代の照合、比較照合更新（compare-and-update）、確定事実の追記（append fact）、利用枠の予約・確定・解放といった低レベル操作をどう型として表現するかを定めます。データベースのトランザクションそのものをビジネスロジック層へ過度に露出させてはなりません。

### 13.1 方針

- **ビジネスロジック層へ `Transaction` オブジェクトを直接露出・受け渡しさせない**:
  リポジトリのメソッド内部で短い即時トランザクション（`Immediate` transaction）、または担当責任者のローカルな逐次化機構と連携し、前提条件の比較照合と永続化データの更新を不可分に実行します。トランザクションの内部で非同期の待機（await）や外部ネットワークI/Oを行ってはなりません（CCT §5）。
- **ビジネス層は前提条件（`expected_*`）と新規内容を渡し、順序保証はリポジトリが担う**:
  最新データの読み直し、前提との比較照合、更新処理の不可分な実行順序はリポジトリが確実に保証します。メソッドの戻り値は、生の更新行数（row count）ではなく、ドメイン判定結果（`CommittedAs`、`StaleExpected`、`MissingTask`、`RevisionExhausted`、`HeldByGlobalHold` など）として型安全に返します。
- **万能な汎用リポジトリや汎用 CRUD インターフェースを安易に作らない**:
  真に同一の不変条件（invariant）を共有する場合にのみ共通化を行います（例：行ごとの比較照合更新の仕組み自体は共有できても、取り扱う型、ライフサイクル、カウンターはドメインごとに明確に区別します）。
- **データベースのテーブルやレコード自身が担当責任者（semantic owner）になるわけではない**:
  意味論的な状態の変更は各ドメインの担当責任者が行い、永続化層は対応関係の安全な保持と照合可能性の提供に専念します（PR §2）。

### 13.2 domain 別の repository interface（pseudo-trait。必要なものだけ abstract する）

実装の差し替え、境界の隔離、テスト容易性（in-memory での再現など）に明確な理由があるインターフェースのみを抽象化します。内部の純粋な計算処理まで無闇にトレイト化してはなりません。

```rust
// --- タスク管理（PR グループD。SD-Task。前提読み取り → 長時間処理 → 短いコミット比較照合） ---
struct TaskPurposeAdoptionPremise {
    purpose: TaskPurpose,            // 採用する目的本文。識別子（identity）は採用位置が保持
    origin: TaskContextOrigin,       // 採用の由来（方針指示 steering では会話: kind = OwnerConversation、source = ProposeSteeringCommand.instruction_source と同一レコード）
    acquired_at: WallClockWithTz,    // 取得時点
}
struct TaskInstructionAdoptionPremise { // 方針指示 steering で採用する追加指示（H-A steering 配線スライスで導入）
    entry: TaskContextEntryId,       // 新リビジョンが記録する採用指示項目の識別子。作業担当（orchestrate）が発行し、項目自身が採用識別子となる
    origin: TaskContextOrigin,       // 採用の由来（kind = OwnerConversation。source = ProposeSteeringCommand.instruction_source）
    acquired_at: WallClockWithTz,    // 取得時点
}
struct TaskCommitPremise {            // 方針指示（steering AU4）。1コミット＝1リビジョン前進
    expected: TaskRef,               // 期待する現在リビジョン。呼び出し側は cur+1 を直接名指ししない
    new_purpose: Option<TaskPurposeAdoptionPremise>, // None = 直前の採用目的識別子・本文を継承。Some は新リビジョンを採用位置とする
    adopted_purpose_entry: TaskContextEntryId, // 新リビジョンが記録する採用目的コンテキスト項目の識別子。作業担当（orchestrate）が発行
    adopted_instruction: Option<TaskInstructionAdoptionPremise>, // この前進（forward）で採用する指示。None = 指示の採用なし
}

enum TaskCommitOutcome {
    CommittedAs(TaskRef),            // コミット成功（新リビジョン。方針指示時は cur+1）
    StaleExpected { current: TaskRef }, // 期待リビジョン不一致（最新リビジョンを添えて返却）
    TaskTerminal { task: TaskId, progress: TaskProgress }, // 対象 Task が terminal（Completed / Failed）。
                                     // 再評価しても前進しないため StaleExpected と区別し、何も書き込まない
    MissingTask { task: TaskId },    // 前提のタスクに対応する永続化状態が存在しない。何も書き込まない
    RevisionExhausted { task: TaskId }, // 次の相異なるリビジョンを永続化データとして確定できない。何も書き込まない
    // HeldByGlobalHold(hold-check premise) は hold スライスで追加（下記参照）
}

// TaskCommitPremise の意味論:
// - 新リビジョン（expected.revision + 1）は、リポジトリが CAS（比較照合更新）成立後に確定します。呼び出し側が未来のリビジョンを直接名指ししてはなりません。
// - new_purpose: Some の場合は採用位置（TaskPurposeRef.adopted_revision）を新リビジョンとし、None の場合は直前の採用識別子・本文を維持して新リビジョンの採用目的項目を再記録します。None の場合、リポジトリは現在リビジョンの採用目的項目から origin および acquired_at を引き継ぎ、同一トランザクション内で読み取ります。
// - adopted_purpose_entry は新リビジョンの採用コンテキスト項目の識別子であり、作業担当（orchestrate）が発行します。リポジトリは渡された識別子を採番し直さず、CAS 成立後に TaskContextEntry.reference = (task, new revision) と、Some の場合の採用リビジョンを刻みます。
// - new_purpose: Some の origin は方針指示の発言レコードとし（kind = OwnerConversation、source = ProposeSteeringCommand.instruction_source と同一レコード）、acquired_at は作業担当が採用時に取得します。目的と指示で由来を分ける生成元が生じた段階で、コマンドに由来フィールドを追加します。
// - adopted_instruction: 方針指示の前進（forward）は採用指示項目を 1 つ記録します（ProposeSteeringCommand.instruction_source は必須）。None は指示を採用しない将来の前進用であり、本スライスの steering 経路では常に Some を渡します。
// - 採用指示の識別子は新リビジョンの TaskContextEntryId であり、リポジトリは渡された識別子を採番し直さず、CAS 成立後に reference = (task, new revision) のみを刻みます。項目は採用リビジョンで 1 度だけ書き込み、以降の前進では再記録しません（目的項目の引き継ぎ再記録とは異なります）。現在有効な採用指示は、現在リビジョンまでの AdoptedInstruction 項目全体となります。
// - 同じ不分区間で現在の task.progress が非 terminal であることを必須とします。terminal（Completed / Failed）は
//   TaskTerminal { task, progress } として何も書き込まずに返し、StaleExpected と区別します（吸収的な終端であり、
//   再評価で前進できる stale ではありません）。この gate は本スライスの契約です。
// - MissingTask / RevisionExhausted / TaskTerminal は正常系（Ok 側）のドメイン判定結果（domain outcome）とし、永続化状態を変更しません。RevisionExhausted は後続リビジョンの不在だけでなく、後続リビジョンを永続化表現に写せない場合も含みます。
// - AU4 のリポジトリスライスが記録するコンテキスト種別は採用目的のみです。最初の追加種別である採用指示は、H-A steering 配線スライスが書き込み側の前提（adopted_instruction）・項目種別識別子（discriminator）・マイグレーション・読み取り規則の拡張を同一の設計変更として追加します。材料や途中理解はそれぞれの利用先が分岐するスライスが同一の形式で追加します。どの種別も同一の forward_steering トランザクションに載せ、別メソッド・別リビジョン・別トランザクションの採用経路を作ってはなりません。
// - HeldByGlobalHold は、hold スライス（HoldConditionRef の生成元が存在する最初のステージ。個別データ削除 / 復元 / 停止のいずれか早い方）において、タスク所有の安全保留照合前提（HoldCheckContextRef 相当）と対で追加します。forward_steering と adopt_result は同一の不可分比較照合内で現在の保留・消去・復元保留を照合し、成立しなければ何も書き込まず HeldByGlobalHold を返します（adopt_result は結果行の記録も保留しません）。それまでは具象 enum に保留の代役バリアントや仮の前提条件を置いてはなりません。この名称は H-A コマンドレベルとリポジトリレベルで共通のドメイン名であり、hold の producer を持つ具象 enum（TaskProposalOutcome / TaskCommitOutcome 等）に hold スライスが追加します。運用管理意図の HeldByOperation（未確定・保留）とは明確に区別される別概念です。

// Task 作成の全内容（AU2）。identity は owner（作業）が発行し、commit 前は委任・実行から不可視。
// 作成は progress = Started で初期化します（呼び出し側は progress を指定しません）。
struct TaskCreationPremise {
    task: TaskId,
    purpose: TaskPurpose,
    entry: TaskContextEntryId,       // 初期 context entry（採用目的）の identity
    origin: TaskContextOrigin,
    acquired_at: WallClockWithTz,
    assignee: AssigneeRef,
    workspace: Option<WorkspaceAssociationPremise>, // 関連付けを確定した場合のみ
}
struct WorkspaceAssociationPremise {
    assoc: WorkspaceAssocId,
    need: WorkspaceNeedRef,
}

// 委任作成の全内容（AU3）。delegation identity と ephemeral identity は owner（作業）が
// 発行し、リポジトリは採番し直しません。delegator は premise では渡さず、リポジトリが
// 同一トランザクションで現在の Task 行の担当者から写します（委任の有効性は常にタスク側の
// 現在単位から再確認し、この写しだけを根拠にしません）。scope_copy は作業担当（orchestrate）
// が依拠した Task 単位の確定済みワークスペース関連付けから写して構成します。
// 委任作成は Task revision を前進させませんが、progress を Started → InProgress へ進めます。
// 同じ不分区間で現在の task.progress が非 terminal であることを必須とし、terminal（Completed / Failed）は
// TaskTerminal { task, progress } として何も書き込まずに返します（terminal 状態は変更しません）。
// 行の存在は実行中・生存の証明ではありません。
struct DelegationCreationPremise {
    delegation: DelegationId,
    task: TaskRef,                   // 期待するタスクリビジョン（boundary token）
    agent: TaskAgentEphemeralId,     // 一時的な実行主体の識別子。行の存在は生存の証明ではない
    scope_copy: DelegationScope,     // 委任時に確定した境界の写し
}

// リロード / 復旧の読み戻し。現行リビジョン単位（AU2 作成 ＋ AU4 前進）（現在リビジョンの目的項目と、
// 現在リビジョンまでの有効な採用指示項目全体）を返します。
struct Task {                        // task 行の現行値（D1）。内容のマスターデータは revision snapshot であり、同じリビジョンを指す
    reference: TaskRef,              // 現在リビジョン
    purpose: TaskPurposeRef,         // 現在採用されている目的（本文は revision snapshot）
    assignee: AssigneeRef,
    progress: TaskProgress,          // 開始 / 進行中 / 完了 / 失敗（revision とは別軸）
    adopted_result: Option<TaskResultId>, // 現在採用されている result。未採用なら None（task_result.adopted_revision から bounded read で解決）
}
struct TaskRevisionRecord {
    reference: TaskRef,
    purpose: TaskPurposeRef,         // このリビジョンで有効な採用目的
    purpose_text: TaskPurpose,       // このリビジョン時点の目的本文スナップショット
    assignee: AssigneeRef,
}
struct WorkspaceAssociation {
    assoc: WorkspaceAssocId,
    task: TaskId,
    folder: WorkspaceFolderRef,
    save_target: Option<WorkspaceFolderRef>,
}
struct TaskRecord {
    task: Task,
    revision: TaskRevisionRecord,    // 現在リビジョンのスナップショット
    context: Vec<TaskContextEntry>,  // 現在リビジョンの目的項目と、reference.revision <= 現在リビジョンの採用指示項目（順序は目的項目が先、次に reference.revision 昇順・同一リビジョン内は entry_id 昇順）。各項目は自身の reference を保持する
    workspace: Option<WorkspaceAssociation>,
}

// 1 つの結果の durable record（D2）。本文は `body` の 1 行だけが正本であり、他の record へ複製しません。
// final result の到着 record が先に本文・identity・execution lifetime を確定し、同じ不分区間で delegation を
// seal します（durable invariant: 1 delegation につき最大 1 行）。採用判定はその同じ行に対して行われます。
// `attempt_refs` は採用判定で authoritative set と完全一致が確認され、`task_result_attempt` に刻印された
// opaque な相関だけを返します（未採用・未判定なら空であり得る。Action owner の newtype ではなく opaque 相関）。
struct TaskResultRecord {
    result: TaskResultId,
    task: TaskRef,                   // 依拠リビジョン（delegation 行の task_revision と一致）
    delegation: DelegationId,        // execution lifetime（1 delegated Task Agent execution）。seal の単位
    body: TaskAgentOutput,           // provider 出力本文（Debug では伏字化）
    attempt_refs: Vec<RawId>,        // 採用判定で検証済みの authoritative set（task_result_attempt の相関）
    adopted_revision: Option<TaskRevision>, // Some は現在 Task へ採用された（adopted result）。None は到着 record のみ
    recorded_at: WallClockWithTz,
}

// 採用判定の主張。caller（orchestrator / Task Agent 経路）が「この result が依拠した」と主張する
// Action 試行の列挙であり、それ自体は完了の根拠になりません。store は result 行の delegation
// （execution lifetime）から authoritative set を列挙し、この集合との完全一致（欠如・追加・重複の
// いずれも不可）を要求します。
struct TaskResultAdoptionClaim {
    result: TaskResultId,
    attempt_refs: Vec<RawId>,
}

trait TaskRepository {
    // タスクの新規作成：task + task_revision + 初期 task_context_entry +（関連付け確定時）
    // workspace_assoc の不可分永続化。コミット前は外部の委任や実行から不可視（durable-before-visible）。
    // いずれかの挿入が失敗した場合は先行の挿入を含めてロールバックし、一部だけの行を可視にしません。
    async fn create_task(
        &self,
        premise: TaskCreationPremise,
    ) -> Result<TaskRef, TaskTechnicalError>;

    // 方針指示（steering）：新リビジョンと新コンテキストを不可分に前進（過去リビジョンも確実に保持）。
    // 採用するコンテキスト項目の識別子は前提条件（premise）が保持し、リポジトリはCAS成立後のリビジョン参照のみを刻みます。
    // 同じ atomic compare で現在の task.progress が非 terminal であることを必須とし、terminal（Completed / Failed）は
    // 何も書き込まず TaskTerminal { task, progress } を返します。
    // hold スライスでは、タスクが所有する安全保留照合前提（hold-check premise）を同一の不可分比較照合（atomic compare）に加えます。
    async fn forward_steering(
        &self,
        premise: TaskCommitPremise,
    ) -> Result<TaskCommitOutcome, TaskTechnicalError>;

    // リロード / 復旧：コミット済みの現行リビジョン単位（AU2 作成 ＋ AU4 前進）を読み戻します。現在リビジョンの
    // 採用目的項目は丁度 1 つ存在し、その採用識別子が Task.purpose および TaskRevisionRecord.purpose と一致しなければ
    // 技術的エラーとします（欠損・複数存在・不一致のいずれの場合も中途半端な TaskRecord を合成してはなりません）。
    // reference.revision <= 現在リビジョンの採用指示項目をデコードして自身の reference のまま返します。
    // 未知の項目種別（unknown item kind）、種別とペイロードの不一致、現在リビジョンを超える項目は技術的エラーとし、
    // 読み飛ばしや勝手な再解釈を行ってはなりません。origin.source は参照であり、参照先レコードの存在や可読性は要求しません
    // （解決できない来歴情報もそのまま返し、本文を複製・再解釈しません）。存在しない識別子の場合は None を返します。
    async fn load_task(
        &self,
        task: TaskId,
    ) -> Result<Option<TaskRecord>, TaskTechnicalError>;

    // 委任の作成：expected_task_revision の比較照合と、現在の task.progress が非 terminal であることを
    // 満たした上で安全に作成し、progress を Started → InProgress へ進めます。delegation と
    // agent の識別子は作業担当（orchestrate）が発行して premise で渡し、リポジトリは採番し
    // 直しません。delegator は同一トランザクション内で現在の Task 行の担当者から写し、
    // 同じリビジョンの task_revision snapshot との担当者一致を確認します（欠如・不一致は
    // 技術的エラー。合成値を作りません）。リビジョン不一致（StaleTaskRevision）、タスク
    // 未存在（MissingTask）、terminal（TaskTerminal { task, progress }）は commit せずに返し、
    // 部分的な行を残しません。
    async fn create_delegation(
        &self,
        premise: DelegationCreationPremise,
    ) -> Result<DelegationOutcome, TaskTechnicalError>;

    // 委任のリロード：delegation_id に対応する対応関係（依拠タスクリビジョン・委任元・
    // ephemeral ID・スコープの写し）を読み戻します。存在しない場合は None。不正な識別子表現・
    // 不正なスコープ表現は技術的エラーとし、合成した値を返しません。行の存在は実行中・生存の
    // 証明ではありません。再起動後の対応解決と、遅延した結果到着の帰属（結果受入スライス）が
    // この読み出しを使い、AU3 スライスのテストは close / reopen 後の再読み出しを検証します。
    async fn load_delegation(
        &self,
        delegation: DelegationId,
    ) -> Result<Option<DelegationRef>, TaskTechnicalError>;

    // 結果の到着 record（durable 化）：Task Agent execution が final result を提出した明示的な finalization 境界で
    // orchestrate が呼び、task_result の 1 回だけの insert とその delegation の seal を短い Immediate
    // トランザクションで確定します。同じ不分区間で delegation 行の存在と `(task_id, task_revision)` の
    // 構造的な妥当性（部分的・不整合な行は技術的エラー）を確認し、欠如は fail closed とします。
    // 保存するのは結果 identity・本文 1 行・delegation 行から解決した依拠 TaskRef・
    // recorded_at であり、adopted_revision は NULL、attempt 相関は書きません（採用判定で検証後 1 回だけ刻印）。
    // durable invariant: 1 delegation につき final task_result は最大 1 行（UNIQUE(delegation_id) 相当）。
    // 同じ delegation に既に final result がある場合、同じ result identity なら本文・委任・依拠リビジョンが
    // 一致するときに限り冪等に COMMIT し、異なる result identity は 2 つ目の final result として受理せず
    // fail closed（技術的エラー）します。本文・委任・依拠リビジョンの不一致を伴う identity 再利用も
    // 技術的エラーとして既存行を上書きしません。現在 revision・progress・確定度・terminal は判定しません
    // （採用・完了は adopt_result の役割）。1 回の inference turn の provider 出力（Produced）は final result では
    // ないため、この step へ入れて seal してはなりません（intermediate output の seal 禁止）。
    async fn record_task_result_arrival(
        &self,
        arrival: TaskAgentResultArrival,
    ) -> Result<(), TaskTechnicalError>;

    // 委任の final result の bounded read：delegation_id に対応する final task_result（0..1 行）を読み戻します。
    // 行の存在そのものがその delegation の execution seal（該当 execution の新しい inference claim / Action 開始を
    // 拒否する根拠）であり、結果 identity・本文・採用リビジョンから再起動後の対応を辿る入口です。存在しない場合は None。
    // 同じ result identity の再 arrival の冪等性確認と、作業側の再読込マッピング（seal 済み → ExecutionSealed）が使います。
    // 部分的な行（本文欠如、採用リビジョンと依拠リビジョンの不整合等）は技術的エラーとし、合成した record を返しません。
    async fn load_delegation_result(
        &self,
        delegation: DelegationId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError>;

    // 結果の採用判定：到着 record 済みの task_result 行に対して、1 つの短い Immediate トランザクションで
    // 判定します。result 行が無い場合は MissingResult（書き込みなし）。まず result 行の delegation
    // （1 delegated Task Agent execution の lifetime）から action_attempt を列挙し、authoritative set
    // （その execution が durable start した Action 試行の完全な集合。seal 時点の membership で固定され、
    // seal 後は増減しない）を解決します。各 attempt の (delegation, task, task_revision) 対応と
    // claim.attempt_refs との完全一致（欠如・追加・重複のいずれも不可）を検証し、不一致は不整合な単位として
    // 技術的エラー（fail closed）とします（推測で stale や Withheld に丸めません）。claim は完了の根拠ではなく、
    // 単独で attempt 集合を狭めたり広げたりできません。
    // 続いて delegation 行を読み、result 行に記録された依拠 TaskRef との対応を確認してから（delegation 行の
    // 欠如は MissingDelegation、対応の不一致は技術的エラー）、現在 task.revision と purpose identity を比較します
    // （purpose text の文字列一致は使いません）。result 行の adopted_revision が Some で、その値が result 行の
    // 依拠リビジョンと一致する場合は、この result 自身が過去に採用・完了を確定した再評価であり、二度目の
    // terminal transition を行わず同じ AdoptedAsCompletion を返します（adopted_revision と依拠リビジョンの
    // 不整合は技術的エラー）。現在 revision が前進済み・task.progress が terminal・
    // 後続 cancel marker あり（marker の producer は後続スライス）の場合は、attempt 相関だけを 1 回だけ
    // 刻印し、現在 Task を変更せず RecordedToOriginalOnly を返します。現在 revision と一致する場合は、
    // 同じ不分区間で Task-wide completion barrier を検証します。barrier は、同じ TaskId に属する全 durable
    // ActionAttempt（全 revision / 全 delegation。steering 前の古い revision の started Action を含む）のうち
    // certainty が Unknown の集合が空であることを要求し、1 件でもあれば Completed への CAS を行いません。
    // barrier の入力は action_attempt の durable facts（task_id / task_revision / delegation_id / certainty）
    // だけで、timestamps・delegation liveness・ephemeral alive flag・result body・自己申告・caller の
    // attempt_refs は使いません（不整合な行は技術的エラーとして fail closed）。result-local authoritative set は
    // 「result が何に依拠したか」の相関であり、barrier は「完了後に未確定の外部作用が確定しないか」の gate で、
    // task_result_attempt に刻印するのは result-local の検証済み集合だけです。cross-delegation / 旧 revision の
    // ConfirmedSuccess / ConfirmedFailure は barrier だけを理由に完了を block しません（ConfirmedFailure が
    // result-local set に含まれる場合だけ既存規則で block します）。inference attempt と unsealed delegation の
    // 存在は barrier に含めません。blockers = authoritative set の ConfirmedSuccess 以外 ∪ Task-wide の Unknown
    // （集合。重複は 1 回）が空でない場合は、attempt 相関（result-local のみ）を刻印した上で
    // WithheldByEffectFacts { attempts: blockers } を返し、Task を完了しません（確定度は実行・拡張 owner の
    // 事実のまま読み取るだけで、Task owner は書き換えません）。
    // membership は result 到着（seal）時点で固定されており、seal 後に ene-action owner の客観的証拠が
    // Unknown を ConfirmedSuccess / ConfirmedFailure へ進めた場合は、この同じ result の再評価で完了が
    // 成立し得ます（membership と certainty を混同せず、membership を増減させません）。
    // authoritative set が durable に空の場合のみ Action 無しとして採用判定へ進みます（claim が空だから
    // Action が無かった、と推定しません）。blockers が空（すべて ConfirmedSuccess、または durable に空）の
    // 場合は、同じトランザクションで task_result.adopted_revision を現在 revision に刻み、task.progress を
    // 非 terminal から Completed へ CAS して AdoptedAsCompletion を返します。
    // 同じ result identity の再 arrival は本文を増やさず、記録済みの adopted_revision と現在状態から
    // 同じ判定を再評価して二重採用・二重完了を防ぎます。delegation 行または task 行が
    // 存在しない場合は MissingDelegation / MissingTask として書き込みなしで返します（識別子不在は
    // 正常系のドメイン判定、部分的・不整合な行は技術的エラー）。
    async fn adopt_result(
        &self,
        claim: TaskResultAdoptionClaim,
    ) -> Result<TaskResultAcceptance, TaskTechnicalError>;

    // 結果の bounded read：result identity から、依拠 TaskRef・delegation（execution lifetime）・本文・
    // 検証済み attempt 相関・採用リビジョンを読み戻します。存在しない場合は None。部分的な行
    // （本文欠如、採用リビジョンと task の不整合等）は技術的エラーとし、合成した record を返しません。
    // 再起動後に結果 ⇔ 委任 / 依拠リビジョン / Action 試行の対応を辿るための入口です。到着 record 済み・
    // 採用未確定（adopted_revision = None）の結果は、本文を保持したまま「到着済み・採用未確定」として
    // 読み戻せます。自動 replay や再採用の根拠にはなりません（ユーザーの明示指示なしに採用・完了しません）。
    async fn load_task_result(
        &self,
        result: TaskResultId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError>;

    // 履歴・コンテキストの追記：現在値の上書きではなく、履歴としての追記（append）。
    async fn append_task_revision_record(
        &self,
        record: TaskRevisionRecord,
    ) -> Result<(), TaskTechnicalError>;
}

- タスク作成の復元世代前提（復元跨ぎタグ）は、値の担当責任者（保全・消去）が存在する開発ステージで `task` に追加します。タスク側は `GenerationInner` を包むタスク所有の前提型とし、他担当の `RestoreGeneration` を直接インポートしません（CI §6.4 の復元跨ぎ参照を欠落させないため）。
- 委任作成のオーケストレーション（作業担当）: `load_task` による事前照合（欠如は `MissingTask`、リビジョン不一致は `StaleTaskRevision { current }`、terminal は `TaskTerminal { task, progress }`）→ `DelegationId`・`TaskAgentEphemeralId` の発行 → `DelegationCreationPremise` の構成 → `create_delegation` 呼び出し、の順に進みます。事前照合は現在性の保証ではなく、`create_delegation` のリビジョン比較照合と terminal gate が事前照合後の競合レースを包含します。リポジトリの結果は変更不能のまま写像します。

// --- アクション試行（PR グループE。SD-Attempt。追記専用 ＋ 行ごとの CAS） ---
// このスライスの read / create / edit は、既存の依拠タスクリビジョンと現在の
// workspace_assoc を前提とします。依拠権限評価・在席・費用予約・消去・復元の前提は、
// それぞれの producer を持つスライスがフィールドと比較を同じ設計変更で追加し、
// それまで代役値や placeholder 列を置きません（AU2/AU14 と同じ規則）。
// 他ドメインの newtype を直接 import せず、素の ID ＋ 世代数値で受け取ります（CM §4.3）。
struct AttemptCommitPremise {
    attempt: ActionAttemptId,        // 識別子はオーケストレーションが発行し、リポジトリは採番しない
    delegation: RawId,               // 依拠する委任対応。1 delegated Task Agent execution の lifetime
                                     // （0..N inference turn / 0..N Action attempt / 0..1 final result）
    task: RawId,                     // 依拠タスク
    task_revision: RevisionInner,    // 依拠タスクリビジョン（(task, revision) の対で運ぶ）
    workspace: RawId,                // 現在の workspace_assoc の識別子
    real_target: RealTargetRef,      // 実行直前に解決された実際の対象
    operation: OperationKind,        // List | Read | Create | Edit（このスライスの closed world）
    relied_evaluation: RawId, // K-B.1 の single-use evaluation の opaque な対応識別子。Permission 所有の semantic newtype そのものではなく、mapping は authorization / orchestration 境界で行う
}

enum ActionStartOutcome {
    Started,                         // 試行行は durable。実行はトランザクション外
    StalePremise,                    // 委任・タスク・ワークスペース前提の欠如/前進。書き込みゼロ
    TaskTerminal,                    // 対象 Task が terminal（Completed / Failed）のため開始しない。書き込みゼロ。
                                     // Action 側は Task lifecycle の値型を import せず、Task owner が自分の
                                     // TaskTerminal { task, progress } として意味を説明する
    ExecutionSealed,                 // delegated execution が final result で seal 済みのため開始しない。書き込みゼロ。
                                     // Task lifecycle の terminal とは別（Task は InProgress のままであり得る）。
                                     // Action 側は Task lifecycle の値型を import しない
}

// 確定度の CAS 結果。Updated 以外は書き込みなしのドメイン結果。
enum CertaintyUpdateOutcome {
    Updated,
    StaleCurrent { current: ActionCertainty },
    MissingAttempt,
}

trait ActionAttemptRepository {
    // 試行の開始：同一の即時トランザクション内で、委任対応、依拠タスクリビジョン、
    // 現在のタスクリビジョン、現在の workspace_assoc、委任の scope_assoc を照合し、
    // 現在の task.progress が非 terminal であり、その delegation が seal 済みでない
    // （その delegation の task_result 行が存在しない）ことを必須として不可分に挿入する（AU5）。
    // terminal（Completed / Failed）は TaskTerminal、seal 済みは ExecutionSealed として
    // 書き込みゼロで返す（両者は別概念。seal は Task が InProgress のままでも成立する）。
    // relied_evaluation は single-use とし、同じ評価 ID での
    // 二度目の挿入は技術的エラーとして拒否する（二重開始を許さない）。
    // 委任行と premise の不一致、複数行の関連付け、未知の操作種別は技術的エラーとし、
    // 推測で stale に丸めない。委任行（execution lifetime）は試行行へ保存され、結果採用の
    // authoritative set 列挙（delegation → action_attempt）の正本になる。seal 前に durable start した
    // 試行は seal の有無にかかわらず試行行に残り、authoritative set から消えない。
    async fn insert_attempt_if_current(
        &self,
        premise: AttemptCommitPremise,
    ) -> Result<ActionStartOutcome, ActionTechnicalError>;

    // 確定度の更新：(attempt_id, expected=Unknown) の比較照合更新（CAS）。
    // unknown → confirmed_success / confirmed_failure、または証拠不能時の
    // unknown → unknown（grounds=outcome_unverified）のみ許可する。
    // 確定済みの値は書き換えない。エージェント自身の自己申告は証拠にできない。
    async fn compare_and_set_certainty(
        &self,
        attempt: ActionAttemptId,
        expected: ActionCertainty,   // 通常は Unknown
        new: ActionCertainty,
        grounds: EffectGrounds,
    ) -> Result<CertaintyUpdateOutcome, ActionTechnicalError>;

    // 再起動後の帰属読み出し：attempt_id を境界にした bounded read。
    // 行の存在は実行中・成功・生存の証明ではない。操作の自動 replay の根拠にしない。
    async fn load_attempt(
        &self,
        attempt: ActionAttemptId,
    ) -> Result<Option<ActionAttemptRecord>, ActionTechnicalError>;
}

// --- 在席管理（PR グループG。SD-Presence。パートナーごとの CAS ＋ 旧→移行中→新） ---
trait PresenceRepository {
    async fn compare_and_begin_transition(
        &self,
        companion: CompanionId,
        expected_generation: PresenceGeneration,
        expected_state: PresenceStateExpectation,
        to_client: ClientId,
    ) -> Result<MoveDecision, PresenceTechnicalError>;

    async fn confirm_transition(
        &self,
        companion: CompanionId,
        transitioning_generation: PresenceGeneration,
        live_check: LiveReachabilityRef, // DBトランザクション外で実施した疎通確認結果
    ) -> Result<PresenceAttributionFact, PresenceTechnicalError>;
}

// --- 費用・利用枠（PR グループF。SD-Cap。予約・確定・解放の3状態管理） ---
trait UsageRepository {
    // 予約レコードの挿入と上限照合を同一トランザクション内で実行（処理中・成否不明の費用をゼロとみなさない）。
    async fn reserve_if_under_cap(
        &self,
        cmd: ReserveUsageCommand,
    ) -> Result<ReservationOutcome, UsageTechnicalError>;

    async fn commit_reserved(
        &self,
        cmd: CommitUsageCommand,
    ) -> Result<(), UsageTechnicalError>;

    async fn release_reserved(
        &self,
        cmd: ReleaseUsageCommand,
    ) -> Result<(), UsageTechnicalError>;
}

// --- 自律学習（PR グループC。形成コミット時に記憶リビジョン・スコープ・制約・消去条件を照合） ---
trait LearningRepository {
    async fn commit_formation_if_current(
        &self,
        target: Option<(LearningId, LearningRevision)>, // None の場合は新規形成
        grounds: SummaryGroundsRef,
        scope: LearningScope,
        constraint: ConstraintSnapshotRef,
        erasure: ErasureConditionRef,
    ) -> Result<FormationDecision, LearningTechnicalError>;

    // 過去リビジョンや根拠は追記（append）として保持し、現在値を上書きしない。
    // 到着順が遅いことを根拠の新旧と混同しない。
    async fn append_revision_record(
        &self,
        record: LearningRevisionRecordRef,
    ) -> Result<(), LearningTechnicalError>;
}

// --- キャラクター適用ポインタ（PR グループA/B。SD-CharApply） ---
trait CharacterApplicationRepository {
    async fn compare_and_set_applied(
        &self,
        candidate: ProposeCharacterApplicationCandidate,
    ) -> Result<CharacterApplicationOutcome, CharacterTechnicalError>;
}

// --- 保全・消去（PR グループJ。SD-Deletion / SD-Restore） ---
trait PreservationRepository {
    // 操作IDおよび消去条件の先行永続化（強制執行より先に永続化する：durable-before-enforce）。
    async fn durable_operation_first(
        &self,
        scope: DeletionScopeDecision,
    ) -> Result<(), PreservationTechnicalError>;

    // 参加者の局所消去完了は、各担当ドメイン側で永続化してから報告される（局所完了だけで全体の安全保留を解除しない）。
    async fn record_participant_completion(
        &self,
        fact: ParticipantCompletionFact,
    ) -> Result<(), PreservationTechnicalError>;

    // システム全域削除完了の不可分な確定：各参加者の局所完了・検証・未完了・失敗状況と、
    // 機械的な残存検証および処理区間内の再到着データの取り込みをすべて満たして確定。完了時に検索トークンを消去。
    async fn declare_global_completion_if_verified(
        &self,
        operation: DeletionOperationId,
        sweep: DeletionSweepGeneration,
        verification: RemainderVerification,
    ) -> Result<GlobalDeletionCompletion, PreservationTechnicalError>;

    // バックアップ復元のマスター切り替え：ステージング検証後、復元世代を更新してマスターポインタを不可分に切り替え。
    async fn switch_restore_generation(
        &self,
        restore: RestoreId,
        expected_generation: RestoreGeneration,
    ) -> Result<SwitchRestoreDecision, PreservationTechnicalError>;
}

// --- 未伝達メッセージ（PR グループB。SD-Undelivered。登録は親と不可分、確定は行ごとの CAS） ---
trait UndeliveredRepository {
    // 会話由来は対話履歴の追記と同一トランザクション、タスク由来はタスク永続化後に別トランザクションで不可分に登録。
    async fn register_if_parent_durable(
        &self,
        fact: RegisterUndeliveredFact,
    ) -> Result<(), UndeliveredTechnicalError>;

    // 画面提示が確認されて初めて Presented とする（durable-after-confirmed。送信成功だけで提示完了にしない）。
    async fn compare_and_mark_reported(
        &self,
        id: UndeliveredId,
        expected: ReportStatus,
        mark: PresentationMark,      // 提示ラウンドと「提示完了／成否不明」の観測事実
    ) -> Result<ReportStatusTransition, UndeliveredTechnicalError>;
}
```

- `*_TechnicalError` はストレージやインフラストラクチャ層の技術的障害を示すものであり、前提不一致（stale）、安全保留（hold）、権限拒絶（deny）、識別子不在（missing）、リビジョン枯渇（exhausted）などとは明確に区別します。`StaleExpected`、`HeldByGlobalHold`、`MissingTask`、`RevisionExhausted` などは、正常系（`Ok` 側）のドメイン判定結果として返します。
- 複数の担当ドメインを跨ぐ更新処理を、単一の巨大なトランザクションや単一のアクターへ無秩序に統合してはなりません。必要なのは短い不可分な読み取り照合（同一 SQLite トランザクションの共有など）であり、各領域の意味論的な変更権限まで統合してはなりません（PR §7）。

## 14. Rust-oriented design — trait / struct / enum の具体化範囲

本書では、具体的な Rust インターフェース設計の基本方針を定めています。ただし、「トレイトが使える場所はすべてトレイトにする」といった過剰な抽象化は厳に慎みます。

| 抽象化するもの（trait 化の対象） | 抽象化を行う正当な理由 | 抽象化しないもの（具象型のまま扱うもの） |
|---|---|---|
| `TaskRepository`、`ActionAttemptRepository`、`PresenceRepository`、`UsageRepository`、`LearningRepository`、`CharacterApplicationRepository`、`PreservationRepository`、`UndeliveredRepository` | 境界の隔離（ビジネス担当者と永続化保持の分離）、比較照合コミットの不可分性のカプセル化、テスト容易性（インメモリのモックによる前提不一致・安全保留・遅延の再現） | 具体的な SQL 文、インデックス設計、マイグレーション手順。DB 製品固有の詳細実装 |
| 推論のトランスポート層（プロバイダプロトコルアダプター） | 未対応プロトコルのプラグインによる拡張、既知プロトコルの直接接続の差し替え、テスト容易性（モックプロバイダによる能力不足・通信エラー・利用量の再現） | 論理的なコンテキスト選択、プロンプト組み立て、文脈圧縮、スコアリング計算。利用元の思考・解釈ロジック |
| 実行・拡張の OS・デバイス・MCP 境界 | サンドボックス、OS デバイス、通信アダプターの差し替え、テスト容易性（モック作用による成否不明の再現） | 操作対象の実名前解決ロジック、外部作用の確信度の意味判断 |
| 観測のキャプチャ・アダプター境界 | 画面キャプチャアダプターの外部コード境界、テスト容易性 | 候補検知の内部スコアリング、ルーティング先の選択アルゴリズム |
| 認証秘密情報の分離保管（OS資格情報ストア抽象） | Windows DPAPI / Linux libsecret / macOS Keychain 等のプラットフォーム別差し替え、秘密情報の非露出の強制 | 利用用途や参照元の意味論、同意や許可の確定判定 |

- **借用（borrowed）と所有（owned）の使い分け**:
  識別子（ID）、リビジョン番号、世代番号、前提トークンは、`Copy` 可能な小さな値または不透明な参照として受け渡し、本文データ、ペイロード、派生成果物は参照IDによって必要な場合にのみ辿ります。対応関係を伝達するためだけに、プライベートな本文データを無駄に複製してはなりません（CI §4.4、PR §2）。
- **非同期メソッド（async method）**:
  リポジトリ、推論呼び出し、外部アクション実行、環境観測などの I/O や長時間処理を伴う境界は `async fn` として定義します。コミットの不可分な実行区間自体は極めて短く保ち、トランザクションの内部で非同期の待機（await）を行ってはなりません（CCT §5）。長時間の作業は前提条件を確認した上で開始し、コミット時に最新状態と比較照合します。
- **型安全な newtype**:
  `CompanionId`、`TaskId`、`TaskRevision`、`PresenceGeneration`、`RestoreGeneration`、`DeletionOperationId`、`DeletionSweepGeneration` などの newtype を厳格に維持します。内部表現が同じ整数型であっても安易な相互変換を許さず、リビジョン番号や世代番号を生の数値として単独で引き回してはなりません。
- **`unsafe` コードの排除**:
  本書で定義した設計構造を表現するために、`unsafe` コードを要求することはありません（CI §2.3）。
- **エラー型の設計**:
  第11節で定めた配置方針に従い、各レイヤーで `thiserror` を用いた明示的なエラー型を定義します。ライブラリの公開エラーとして、型のない生の `String` や `Box<dyn Error>` を使用してはなりません。

## 15. IPC readiness — process / network boundary を越える可能性がある interface

通信スキーマの完全な仕様は [Host↔Client IPC](host-client-ipc.md) にて定めます。本節では、プロセス内完結のインターフェースと、ネットワークやプロセス境界を越える可能性があるインターフェースの境界を明確にします。

| 通信区分 | インターフェース | 境界を越えるもの／越境を許容する理由 |
|---|---|---|
| リモート越境可能（Host↔Client） | X-B 対話ラウンド受付・提示・区切り（`SubmitClientInputCandidate` / `RoundIntakeOutcome` / 提示確認の `PresentationMark`） | クライアントは入力や画面表示の一時的な表現のみを持ち、ホストのマスターデータは保持しません。クライアントからのメッセージは単なる「提案候補」であり、ホスト側の最新の在席帰属、実行許可、安全保留との照合が必須です。未送信の操作を勝手にキューイングしたり、クライアント側のデータでホストを上書きしてはなりません |
| リモート越境可能 | X-A 在席移動・復帰（`RequestMoveCommand` / `PresenceAttributionFact`） | 呼び出しや移動の意図はクライアントから送られてきますが、在席の正式な成立はホスト側の帰属記録で決定します。表示ヒントや復旧先候補の存在だけで勝手に在席を成立させてはなりません |
| リモート越境可能 | X-H 未伝達メッセージの次回接続端末への報告（`UndeliveredSummaryFact`） | 新しい端末での要約報告は、ホスト側の元記録、利用制限、データ削除状況と照合された派生表現です。接続や表示データの送信成功だけで「報告完了」と誤認してはなりません |
| リモート越境可能 | X-D 観測資格通知の一部（`NotifyPresenceChangeFact` のクライアント向け表示データ） | 観測状態や対象範囲の説明可能性を担保するために伝達します。Rawデータ、検知候補、ルーティング用データ、プライベートなコンテキストを直接送信してはなりません |
| リモート越境可能 | D-B 個人データ完全削除へのクライアント一時データの参加（`DemandLocalErasureCommand` のクライアント宛て要求） | 接続中のクライアントに存在する一時キャッシュや拡張の一時データも消去に参加します。ネットワーク切断や無応答を「消去成功」と誤認せず、未確認範囲を保全・消去担当へ正しく伝えます。再接続時に古いデータをホストへ持ち帰らせてはなりません |
| リモート越境可能 | C-A キャラクター表示資材の利用（Body／Voice 資材） | キャラクターの静的な設定資材の画面表示・音声再生用の利用。適用関係や成長した経験データは送信しません。ホスト内部の主キーとして再利用できる形式で渡してはなりません |
| リモート越境可能（高権限の最終確認を除く） | 管理画面の表示・操作（第9節の `ManagementOperationCommand` のクライアント側入力） | 管理操作の意味解釈、適用、成否判定は担当責任者が行います。重大な操作要求をリモートから送信できたとしても、最終確認はホストPC本体の信頼できる画面上で行います（IPC §18）。UI層にシステム状態を勝手に書き換える特権を与えてはなりません |
| ホスト内部完結（越境禁止） | H-B〜H-E の自律学習の形成・訂正・スコープ判断、K-A〜K-C の制御確定・秘密利用、K-D〜K-G のプロバイダ割り当て・送信条件・利用枠予約、K-H の認可・実作用確定、D-A・D-C・D-D の削除範囲確定・完了確定・復元切り替え | 最新前提の照合、認証秘密情報、世代番号、データ消去条件をホスト側のマスターデータとして厳格に確定させるため。クライアント、プロバイダ、MCPサーバー、プラグインをマスターデータ保持者や担当責任者にしてはなりません。解決済みの経路、判定のコピー、クライアントの自己申告を、そのまま外部の決定権威にしてはなりません |
| ホスト内部完結（越境禁止） | 第13節のリポジトリ比較照合コミット群（Repository compare-and-commit） | 永続化データの比較照合を、ホスト内部の単一の不可分なトランザクションとして完了させるため。データベースの低レベルトランザクションをクライアントや外部拡張へ露出させてはなりません |
| 外部境界（Host→Provider／MCP／Plugin／外部ファイル） | K-E のプロバイダ送信、K-I の外部拡張利用、K-C の認証用途供給、C-C のパッケージインポート受け入れ | 利用目的、対象データ、送信先、利用同意、費用上限、秘密保護の条件を厳格に課した上で通信します。外部のプロバイダやMCPサーバーをドメインの担当責任者にしてはならず、秘密情報そのものを通常の通信経路へ流出してはなりません |

- ホスト側の永続化、ホスト↔クライアント間の通信、および監査ログ・デバッグ記録では、識別子、リビジョン番号、世代番号、相関IDを明示的なフィールドとしてシリアライズします。自由記述のテキスト本文中に埋め込まれた文字列を、同一性の照合に使ってはなりません（CI §4.6）。
- クライアント、プロバイダ、MCPサーバー、プラグインとの境界では、ホスト内部の主キーや内部ID体系をそのまま露出する必要はありません。露出する場合であっても用途を限定した不透明な参照IDに留め、内部のマスター主キーとして再利用できる形式で渡してはなりません。また、認証情報の秘密値そのものを通常の通信経路に載せてはなりません（DR-05）。
- 日時データは、実時間（wall-clock）と作成時のタイムゾーンをセットで保持します。スケジュール設定においてタイムゾーンを暗黙のうちに勝手に変換してはなりません。

## 16. Crate / module decomposition への材料 — 依存方向

具体的なクレート構成は [クレート・モジュール分割](crate-module-decomposition.md) にて定めます。本節では、意味論的な依存関係（semantic dependency）とインターフェースの呼び出し方向を確定します。

### 16.1 依存方向表（caller → owner の interface 依存）

※ 矢印はアーキテクチャ上の依存関係（DR §1）を示しており、クレートやプロセスへの単純な1対1の直写ではありません。B の処理結果が A へ返ってくることだけを理由にして、逆向きの依存関係を追加してはなりません。一方、B が処理の判断材料として A の活動状態を必要とする場合は、別の独立した参照依存として明記します。

| 呼び出し側（要求・利用元） | → 担当責任者（確定・意味論） | 使用インターフェース | 依存関係の責務・役割 |
|---|---|---|---|
| 個体調整 | → 作業 | H-A（タスク化／方針指示／中断） | 状態変更の要求 ＋ 進捗・結果の参照。タスクの受理や達成判断は作業担当が行う |
| 個体調整・作業 | → 認識・学習 | H-B / H-C / H-D / H-E（経験提出／訂正／参照／スコープ判断） | 状態変更の要求 ＋ 利用範囲内での参照。知識形成やスコープ判断は認識・学習担当が行う |
| 個体調整（仲介役） | → 各元の担当責任者 | H-F（ルーティング文脈供給） | 範囲を限定した参照。タスクの所有権や包括的なアクセス権を移譲するものではない |
| 個体調整 | → 接続・存在 | X-A（在席移動要求） | ライフサイクルの協調（移動の必要性と在席の成立の分離）。排他的な在席帰属は接続・存在担当が決める |
| 入出力・提示・共有観測・実行・拡張 | → 接続・存在 | X-A・X-D・X-F（帰属・接続状態・可用性の参照） | 状態の参照。在席していることだけを根拠にしてアクション実行許可を導いてはならない |
| 接続・存在 | → 入出力・提示・実行・拡張 | X-F（対話区切り・停止結果・成否不明の利用） | ライフサイクルの協調。ホスト側タスク全体の終了を端末切り替えの前提にしてはならない |
| 共有観測 | → 個体調整・認識・学習 | H-F・X-E（限定文脈の取得・候補の意味判断要求） | 範囲を限定した参照。最終的なアクション実行の意思決定を観測側が行ってはならない |
| 個体調整・作業・認識・学習・共有観測・入出力・提示 | → 推論 | K-E（推論利用要求） | 実行要求 ＋ 能力・利用量の参照。生成結果の意味論的な解釈は利用元が行う |
| 個体調整・作業 | → 実行・拡張 | K-H・K-I・K-J（外部作用・拡張・端末依存作用の要求） | 外部作用の実行要求。権限の認可や作用成功の自己申告を行ってはならない |
| 各利用・保存・実行箇所 | → 権限・制約 | K-A・K-B・K-F・K-G（意図確定・リアルタイム照合・フォールバック・利用枠可否） | 制約の強制執行。各ドメインの意味論を決定したり、権限を自己変更してはならない |
| 権限・制約 | → 個体調整・作業・接続・存在・認識・学習・推論・保全・消去 | K-B の前提参照（活動状態・委任範囲・帰属・スコープ・利用量・保留の参照） | 判断に必要な範囲に限定した状態参照。参照先データを勝手に更新してはならない |
| 推論・実行・拡張・接続・存在 | → 認証秘密情報 | K-C（認証用途の利用要求） | 認証処理の代行利用。秘密値そのものを一般参照してはならない |
| 各担当責任者 | → 保全・消去 | D-A〜D-E（操作範囲・未完了・安全保留の依存） | データ消去、バックアップ、復元への参加。保全担当に任意データの直接編集権を与えない |
| 保全・消去 | → 各参加担当者 | D-B（対象特定・局所消去・検証の要求） | ライフサイクルの協調（目的限定）。保全担当が各ドメインのマスターデータ保持者になってはならない |
| キャラクター利用側（個体調整） | → Character | C-A（静的資材・リビジョンの利用） | 資材データの参照。適用関係やパートナー固有の経験状態をキャラクター側へ持ち込まない |
| オーナー（管理経路） | → 各担当責任者 | 第9節・H-H・C-C・C-D・D-A・D-D・D-E | 状態変更の要求 ＋ ライフサイクル協調。パートナーの思考LLMや長時間タスクを介在させない |

- 必要な双方向の参照関係は、目的を限定して安全に維持します（DR-12）。たとえば、権限・制約担当は作業担当の委任範囲や推論担当の利用実績を読み取り、作業担当や推論担当は権限側の最新の制約に従います。事実の記録を残すために次のアクション実行許可を待つ必要はなく、制約の判断のために審査対象のアクションを先に実行してしまうこともありません。循環した承認待ちや成功待ちを前提とせず、グラフを一方向に見せるためだけの無駄な汎用抽象化レイヤーは導入しません。
- システム全体を統制するような新しい汎用仲介者（mediator）、統合コンテキスト層、グローバル設定マネージャー、統合永続化マネージャーなどを追加してはなりません（DR §1）。

### 16.2 分解への制約

- 上記の「呼び出し側 → 担当責任者」の方向性が、モジュールおよびクレートの依存関係を設計する上での第一のインプットとなります。担当責任者側が呼び出し側の内部実装を知るような逆依存を作ってはなりません（例：作業担当が個体調整担当の会話履歴内部を勝手に所有しない、認識・学習担当がタスク記録を所有しない、権限担当が学習データの全文を取得しない、保全・消去担当が各ドメインの内部データを任意に書き換える権限を持たない）。
- 同一の担当領域内における責務の分割（例：推論、作業、自律学習、権限、実行、接続、入出力、観測、保全の各内部責務）であっても、それぞれが異なる状態とライフサイクルを持つため、これらを単一の状態構造体、単一のトレイト、単一のテーブル、単一のアクターへ安易に統合してはなりません。
- 同一のストレージ技術を共有していること（例：単一の SQLite データベースファイルを利用すること）は、担当責任者の境界を統合する理由にはなりません。トランザクションの共有は下位の実装手段（mechanism）に過ぎず、ビジネス上の所有権（ownership）とは無関係です。

## 17. Validation — interface だけを追う walkthrough

具体的な実装コードを想像しなくても、必要な識別子、前提条件、処理結果の意味論がインターフェース上で確実に保たれていることを確認します。各シナリオは、インターフェースID、受け渡される境界トークン、担当責任者の確定判断、および返却される判定結果のみを追跡します。

### V-1 Owner text → Companion response → Learning（H-B・H-D・K-E・K-B）

1. 入出力・提示担当が `SubmitClientInputCandidate(companion, client, claimed_generation, round)` を個体調整担当へ渡します。クライアントからのメッセージだけで在席は成立しません。個体調整担当は X-B の `RoundIntakeOutcome` を経て、現在の対話ラウンドとして正式に受理します。古いラウンドへの遅延入力であれば `StaleRound` として元の過去ラウンドに対応付け、勝手に新しいラウンドへ付け替えてはなりません。
2. 個体調整担当は「応答生成」という利用目的と論理コンテキストを定め、認識・学習担当へ `LearningQuery(purpose=返答, scope_need, constraints)` を発行して利用可能な知識を問い合わせます。クエリの取得成功は、後続の外部送信許可を意味しません。
3. 応答生成のための推論は、`AdmissionRequest(consumer=CompanionReasoning, 用途, 論理コンテキスト)` の認可と試行の確定によって前提条件を確定し（K-B の単一利用認可を含む）、プロバイダへ送信します。同意不足、費用上限超過、安全保留、在席世代不一致、消去条件への抵触があれば、送信前に `NotSent` として処理を安全に中断し、オーナーの指示待ちへ戻します。
4. 応答生成後、個体調整担当および作業担当は、`ProposeExperienceCandidate(experiencer, source_range, source_kind=対話, intended_use=Learning候補)` を認識・学習担当へ提出します。生の本文の無駄な複製は要求しません。
5. 認識・学習担当は `FormationDecision` を確定します。長期保存する価値がなければ処理を終了し、すべての対話を無理に保存することはありません。知識を形成する場合は要約根拠（`SummaryGroundsRef`）を紐付け、必要な記憶データのみを安全に更新します。ユーザーへの応答生成の完了と、バックグラウンドでの学習データ更新完了を同一視してはなりません。
6. **失われてはならない情報**: 経験の発生由来の区別、対象パートナー・タスク・委任との関係、想定される利用用途（応答用か学習用かの別）。「到着が最後だから最新の根拠である」と誤認してはなりません。

### V-2 Task creation → Task Agent → steering → result（H-A・K-H・K-K）

1. 個体調整担当が `ProposeTaskCommand(requester, purpose, origin, workspace_need)` を作業担当へ渡します。会話上での受付は、タスク本体への反映ではありません。作業担当は `TaskProposalOutcome::AcceptedAsTask(TaskRef)` を確定します。タスクの作成は、`TaskCreationPremise`（初期目的・初期コンテキスト項目・確定したワークスペース関連付け）が不可分に永続化された後に外部へ可視化されます。
2. 作業担当は `CreateDelegationCommand(task=TaskRef(expected), scope_copy)` を発行して一時エージェントへの委任を作成します。作成は、期待リビジョンと現在のタスクリビジョンの不可分な比較照合（AU3）と `task.progress` が非 terminal であることを満たした場合にのみ `DelegationRef` として永続化され、progress を `Started → InProgress` へ進めます。リビジョン不一致は `StaleTaskRevision { current }`、terminal は `TaskTerminal { task, progress }` として書き込みなしに再評価へ戻ります。エージェントは一時的な従属主体に留まり、独立した特権、認証情報、プロバイダ設定の上書き権限、個別の予算枠限度を持ちません。`consumer_assignment` は保存された identity としては導入せず、Task Agent の推論受付時に委任元が依拠する現在の Capability 同意から live に解決します（durable な帰属は推論試行行の `(consumer, purpose, delegation, 依拠 TaskRef)` が担います）。
3. オーナーからの追加指示は、`ProposeSteeringCommand(premise=SteeringPremiseRef, new_purpose, instruction_source)`（`unadopted` フィールドは W-3 の反映判断を生成元に持つスライスで導入）によって、新リビジョンと新コンテキストの不可分な前進（forward）となります。新リビジョンは採用目的項目と採用指示項目を同一の `forward_steering` トランザクションに記録します。採用識別子は作業担当が確定し、採用指示項目自身の `TaskContextEntryId` が表します（`instruction_source` は由来レコードの参照であり、採用識別子そのものではありません）。過去のリビジョンも確実に保持されます。2つの方針指示が競合した場合は同期区分 SD-Task の順序で直列化され、先に確定した方を優先して現在の状態とし、後から到着した要求は新しい現在の状態に対する再指示として評価します。
4. 方針指示（steering）が行われた後に、古いリビジョンを前提とした委任作成やアクション実行要求が届いた場合は、`StalePremise { current }` または `StaleTaskRevision { current }` として不受理にし、再評価へ差し戻します。Task が terminal の場合は revision stale と区別し、委任は `TaskTerminal { task, progress }`、steering は `TaskProposalOutcome::TaskTerminal`、Action は `ActionStartOutcome::TaskTerminal` として書き込み・開始ともに行いません。実行中だった古い委任はベストエフォートで停止・縮小させ、古い結果を勝手に新しい目的に採用してはなりません。Task Agent の推論開始も同じ照合に従い、依拠リビジョン・現在のタスクリビジョン・`task.progress` が非 terminal であること・delegation が seal 済みでないことの比較を推論試行の確定と同一の不分区間で行います（照合に失敗した新規推論は `TaskPremiseStale` として送信前に拒絶し、作業側の再読込が terminal を検出した場合は `TaskTerminal`、delegation の final result（seal）を検出した場合は `ExecutionSealed` へ写します）。
5. エージェントの処理結果は、Task Agent execution が final result を提出した明示的な finalization 境界で `TaskAgentResultArrival(delegation, result, body)` として、orchestrate が `TaskResultId` を発行し final result の可視化より前に `record_task_result_arrival` で durable に記録します（本文を失わない。同じ不分区間で delegation を seal し、1 delegation につき final result は最大 1 つ）。`TaskAgentTurnOutcome::Produced` は 1 回の inference turn の provider 出力であり、Action 要求や途中経過を含むため final result でも seal でもなく、到着 record に入れません。その後の採用判定は `TaskResultAdoptionClaim(result, attempt_refs)` を `adopt_result` が 1 つの短い `Immediate` トランザクションで判定します。依拠リビジョンは delegation 行から解決し、その revision snapshot の目的 identity と現在の目的 identity を比較します（本文の文字列一致は使いません）。同時に result 行の delegation（execution lifetime）から Action attempt の authoritative set を列挙し、claim の `attempt_refs` との完全一致を要求します（欠如・追加・重複は技術的エラー。seal 後に membership は変化しません）。現在リビジョンが前進済みまたは Task が terminal の場合は `RecordedToOriginalOnly` として元の依拠リビジョンにのみ記録し、現在のタスクには不採用とします。現在リビジョンと一致しても、result-local authoritative set に `ConfirmedSuccess` 以外があれば、または同じ TaskId の全 revision / 全 delegation の Action 試行に `Unknown` が残れば（Task-wide completion barrier。blockers は両者の和集合とし、重複は 1 回）、`WithheldByEffectFacts` として完了しません（barrier で見つけた試行は result-local 相関として `task_result_attempt` に刻印せず、terminal CAS の可否だけを判定します）。古い承認情報を使ってタスクの中断を勝手に解除してはなりません。採用・競合・不明の詳細は V-10 で確認します。
6. **失われてはならない情報**: 単なる発言記録とタスク反映内容および未反映・保留指示の対応関係、方針指示の前後の目的の区別、委任範囲、ワークスペース境界、クライアント依存条件、タスクリビジョンの前提情報。

### V-3 Action candidate → authorization → external effect → timeout → late result（K-H・K-B・K-K・H-A）

1. 機能利用元が `ActionCandidate(principal_chain, task, delegation, workspace, purpose, described_target, operation, data_use, cost_risk, relied_intent_rule)` を組み立てます。候補構造体を組み立てられたこと自体は、実行許可を意味しません。
2. 実行・拡張担当は現在の関連付けのフォルダから対象を実行直前に解決し（canonical 化・フォルダ配下の照合・mount/reparse 境界の照合。`List` は directory、`Read`/`Edit` は regular file）、`RealTargetRef` を得ます。続いて `ActionUseCandidate(operation, resolved_target, delegation, task, task_revision, workspace)` を K-B.1 の `authorize_action_use` に渡します。`AllowForThisUse` の評価 ID だけが開始を許し、`Deny` / `NeedsRevalidation` は書き込みも実行も行いません。推論用の `PermissionEvaluationId` / `EvaluationTracker` は流用しません。
3. `AllowForThisUse` の評価 ID を consume してその opaque な `RawId` に落とし、`AttemptCommitPremise(attempt, delegation, task, task_revision, workspace, real_target, operation, relied_evaluation)` を `insert_attempt_if_current` に渡します。同一の短い `Immediate` トランザクションで委任対応・依拠タスクリビジョン・現在のタスクリビジョン・現在の `workspace_assoc`・委任の `scope_assoc` を照合し、現在の `task.progress` が非 terminal であり、その delegation が seal 済みでないことを必須とし（AU5）、同じ評価 ID の二度目の挿入は技術的エラーとして拒否します。`Started` の場合のみトランザクション外で解決済みの対象に作用し、欠如・前進は `StalePremise`、terminal は `TaskTerminal`、seal 済みは `ExecutionSealed` として書き込みも実行も行いません。
4. 作用の確定度は実行・拡張担当が実行そのものの観測から記録します。`ConfirmedSuccess` は対象の事後確認（例: 書き込み後の読み戻し一致）を根拠とし、エージェント自身の「成功しました」という自己申告を証拠にしません。作用が起き得なかったことが確認できた場合は `ConfirmedFailure`、確認できない場合は `Unknown` のまま保持し、中断要求の受理、通信の成功、画面への表示、DB保存の成功、再接続、復元、端末移動などを理由にして勝手に成功や失敗へ書き換えません。試行行は開始時に `Unknown` で挿入され、確定度は行ごとの CAS でのみ更新します。
5. 遅れて届いた成功の証拠は、元の試行記録に対して `compare_and_set_certainty(attempt, expected=Unknown, new=Confirmed, grounds)` のアトミック更新（CAS）で記録します。新たな客観的証拠の確認を必須とし、エージェント自身の自己申告を証拠にしてはなりません。現在のタスクへ結果を採用するかどうかは、H-A の受付インターフェースにおいて改めて判定します（このスライスは採用を行いません）。成否不明となったアクションを再実行するには、新しい試行IDの発行と、重複リスクを明示したオーナー自身の再判断が必要です。
6. **失われてはならない情報**: 判定時の想定対象と解決済み実対象の対応関係、委任の不変性、確定度（確認済み成功／確認済み失敗／成否不明）、試行と外部作用の区別、遅延到着の帰属、重複実行防止に必要な永続記録。

### V-4 Provider request near cap → reservation → usage（K-D・K-G・K-E・K-B）

1. 利用元が用途と論理コンテキストを定め、推論担当が `ResolveAssignmentQuery(consumer, capability, current_consent)` を用いてプロバイダ経路を解決します。解決された経路は単なる派生データであり、設定変更や同意の更新があった後も過去の選択を勝手に有効とみなす根拠にはなりません。
2. 各利用担当者は `ReserveUsageCommand(consumer, cap, upper_bound, attribution)` を発行して利用枠を事前に予約します。同一のトランザクション内で利用枠上限と関連する利用実績（予約中 ＋ 確定済み ＋ 成否不明の合計値）を読み取って照合します（SD-Cap）。上限を超過している場合や、費用が不明で安全に継続できない場合は処理を開始しません。
3. 予約完了後の推論呼び出しや外部ツール実行は、データベースのロックを持たずに並行して実行されます（他の要求をブロックしません）。実行中に認証失効、安全停止、保留が発生した場合は、ベストエフォートで処理を停止します。
4. 処理完了時は `CommitUsageCommand(reservation, actual=確定値／不明の区別)` によって不可分に更新し、未使用の予約枠を解放します。遅延して届いた利用量報告は、元の予約ID、試行ID、タスク、割り当て設定へ正しく帰属させます。未報告の利用量、処理中の利用、成否不明の消費を、勝手にゼロやリセットにしてはなりません（プロセスの終了、端末移動、キャッシュ消去、ログ整理の際にもリセットしません）。
5. 共有観測による画面キャプチャの検知処理は、対象クライアントおよび観測専用割り当てにおける単一の利用として計上し、端末にいるパートナーの人数分を二重計上しません。配信後に各パートナーが個別に行う推論処理は、独立した新たな実利用として同一の全体利用枠へ含めます。
6. **失われてはならない情報**: 報告された実測値、成否不明、処理中費用の厳格な区別、消費主体・用途・送信先の対応関係、同一残額の二重消費の防止、利用枠上限超過や費用不明時におけるデータを保持したままでの安全停止・判断待ち状態。

### V-5 Client summon → move → stale old Client input（X-A・X-B・X-F）

1. 個体調整担当が移動意図（オーナーの明示的呼び出し、事前指示、文脈に応じた自発的移動）を、`RequestMoveCommand(companion, from, to, reason, expected_generation, expected_state, round_closure)` によって接続・存在担当へ伝達します。移動の必要性の判断と、在席の成立は別です。
2. 接続・存在担当は、`expected_generation ＋ expected_state` のアトミック更新（CAS）によって、「旧端末 → 移行中 → 新端末」の永続状態遷移を確定します（SD-Presence）。ほぼ同時に複数の端末から呼び出しが競合した場合は、先に確定した要求のみを成立させ、後から到着した要求は不受理として再評価へ戻します。移行中の過渡状態では、新旧どちらの端末でも端末依存の新規処理を開始してはなりません。
3. 古い端末から遅れて届いた入力や未提示の出力は、`StaleRound` として元の過去ラウンド、元の在席帰属、元の試行記録に対応付け、勝手に新しいラウンドへ付け替えてはなりません。古い端末で生成済みだった未提示の出力は未伝達メッセージ管理へ引き継がれ、新しい端末において現在の最新結果や利用制限に基づいて要約報告されます。
4. 新しい端末からの入力は、新しいラウンドおよび新しい試行としてのみ受け付けます。正式な在席が成立する前の新しい端末からの入力によって勝手に在席を成立させてはならず、過去ラウンドの継続として実行してはなりません。また、再接続したクライアントが保持していた古い一時状態だけを根拠にして、現在の在席、実行権限、処理再開を成立させてはなりません。
5. **失われてはならない情報**: 在席状態、ホスト側の継続処理、現行接続、実行権限の区別、端末切り替え期間の明確な区別、二重在席の禁止、安全な対話区切り、古い端末のアクションを別端末で自動継続することの禁止。

### V-6 shared Observation → routing → Companion reaction（H-F・X-D・X-E・K-E）

1. 接続・存在担当が `NotifyPresenceChangeFact(client, present_companions, fullscreen, observer_control)` を共有観測担当へ通知します。停止中（Stopped）のパートナーを在席人数やルーティング対象として数えてはなりません。以前の端末で取得された古い画面キャプチャや分析候補を使って、新しいキャプチャや情報配信を続けてはなりません。
2. 共有観測担当は対象とタイミングを確定し（`EligibilityDecision`）、`PublishObservationCandidate(client, candidate, captured_at, capture_range, routing_context=H-F)` によって候補を検知します。これは観測専用の割り当て設定を用いて実行し、パートナー側の設定の上書きや同意の合成を行ってはなりません。全パートナーへの無条件な一斉同報配信や、パートナーごとの重複した無駄な検知処理を行ってはなりません。
3. 共有観測担当は `RoutingDecision::RoutedTo(該当端末でアクティブに動作している関連パートナーのみ)` を確定し、`DeliverEventNotification(candidate, target, deliverable_range)` によって通知を伝達します。通知を受信したことと、内容を理解・採用したことは別です。
4. 各パートナーの個体調整担当は、`CompanionUnderstandingFact(companion, candidate, understood_as, experience_link)` によって、自身の文脈に照らし合わせた意味解釈を行います。共有観測によるルーティングは、各パートナー自身の意味判断、発話判断、アクション実行判断を勝手に代行するものではありません。発話やアクションを行うかどうかの最終判断は、各パートナーが自身のキャラクター設定、親密な関係性、状況、ルールに基づいて自律的に行います。
5. 配信後に各パートナーが個別に行う推論処理は、独立した新たな利用として K-E および K-B の認可フローを改めて経る必要があります。共有観測で外部へ送信できた画面データや背景情報を、そのまま各パートナーのプロバイダへ無条件に送信できるとみなしてはなりません。
6. **失われてはならない情報**: 端末単位の画面共有と各パートナー個別の意思決定の分離、ルーティング用コンテキストの派生性（新しいマスターデータや全体共有データではないこと）、元データの利用制約と観測専用割り当ての送信同意の維持、スコープ変更・同意失効・データ削除要求の処理中派生データへの即時反映。

### V-7 Targeted Deletion 中の delayed Learning result（D-A・D-B・H-B・K-K）

1. 保全・消去担当が `RequestTargetedDeletionCommand(purpose, mechanical_condition, semantic_hint, exclusion)` を受領し、`DeletionScopeDecision(operation, sweep, valid_interval, participants)` を確定します。消去操作レコードおよび消去条件の永続化は、各参加者が消去処理を開始するよりも前に完了させます（durable-before-enforce）。
2. 各受入・保存先は、データの保存や採用をコミットする直前に、処理中に発行された消去条件（`ErasureConditionRef(operation, sweep, valid_interval)`）をロックを持たずに高速に照合します。到着・生成された情報について、「元データとの関係および生成日時」と「消去操作の有効期間」を突き合わせて判定します。削除処理区間内に遅延到着したり再生成されたデータはすべて消去対象とし、新しい経験データとして救済・保持してはなりません。
3. 削除開始前に開始されていた自律学習の形成処理から遅れて結果が届いた場合、通常の前提条件（最新の記憶リビジョン、スコープ、制約）に加えて消去条件を厳格に照合します。削除対象の由来を持つ遅延結果は、過去ログへの事実記録に留めるか安全に破棄し、現在の認識への採用や再保存を行ってはなりません。削除対象となった情報を利用して、実行中の学習処理がデータを再保存することを確実に防止します。
4. システム全域での削除完了は、`declare_global_completion_if_verified(operation, sweep, verification=NoRemainderMechanically ＋ 区間内再到着の取り込み)` によって不可分に確定します。一部の参加者からの局所的な完了報告だけで安全保留を解除してはなりません。未確認の領域や到達不能な端末が存在する状態を「成功」と誤認してはなりません。完了時には機械的条件の検索用トークンを完全に消去（wipe）し、完了記録や監査ログに対象の本文データを書き戻してはなりません。
5. **失われてはならない情報**: 機械的な必須条件レイヤーと意味的な補助ヒントレイヤーの区別、消去処理区間、完了境界、安全保留状態、遅延結果の帰属と再保存防止措置、削除前の由来を持つデータと削除完了後に新しく提供されたデータの厳格な区別。

### V-8 Restore 後の old live result（D-D・K-K・H-A）

1. 保全・消去担当が `RequestRestoreCommand(backup_point)` を受け、`StagedRestoreCandidate(restore, backup_point, isolation_check)` によってステージング領域（別ファイルまたは別グループ）での整合性照合と検証を先行して実行します。ステージング検証を行っている期間中も、本番環境の更新は旧マスターデータに対する通常のコミットとして継続され、ステージング領域の内容と混ざり合うことはありません。
2. 検証完了後、`SwitchRestoreDecision(restore, new_generation, hold)` によって復元世代番号（`restore_generation`）を更新し、マスターポインタを不可分に切り替えます（SD-Restore）。切り替え前は「復元前の正常状態」がマスターデータであり、切り替え後は「復元されたデータ」がマスターデータとなります。両者が混ざり合った第三の状態を作ってはなりません。復元に失敗した場合は、復元前の正常な状態をそのまま維持します。
3. 切り替え処理の前に開始されていた推論呼び出しや外部ツールの実行結果が切り替え後に遅れて届いたとしても、「処理開始時の復元世代前提、割り当て・同意リビジョン、認証情報、依拠したルール」と「最新の復元世代、現在の保管庫、最新の制約、復元後の一括安全保留」を照合することで安全に遮断します。復元前の古い実行要求、古い同意、古い割り当て設定だけで、自動処理を勝手に開始してはなりません。復元前の古い実行要求、処理結果、クライアント側のキャッシュコピーを、新しく復元されたマスターデータへ混入させてはなりません。
4. バックアップ復元を跨ぐ参照情報（タスク、ルール、同意、プロバイダ割り当て、外部作用、未伝達メッセージ、システム全域の未完了状態）には、必ず復元世代番号（`RestoreGeneration`）を付与します。古い世代の参照情報だけで、復元後に外部処理を実行・送信してはなりません。なお、タスクの方針変更はタスクリビジョンを進めることで表現し、復元世代番号の変更で代替してはなりません。
5. 復元が正常に完了した後も、タスク、スケジュール、外部接続を伴う自動処理は安全のため保留状態を維持します。オーナーが画面上で復元内容を確認した後、`BulkEnableAfterRestoreCommand` によって安全にまとめて有効化できます。現在の最新の認証情報、現在の制約、復元後保留、拒絶設定、費用上限、認証不足、成否不明の状態を無視して処理を進めてはなりません。
6. **失われてはならない情報**: 復元対象範囲、マスター切り替え、復元後の一括再有効化の分離、過去の古い状態を現在の事実と誤認することの禁止、復元前データの混入禁止、現在の認証情報保管庫の維持、外部ファイルや外部作用が巻き戻らないことの認識、単一のマスターデータの維持、復元後の一括安全保留。

### V-9 Character revision apply（C-A・C-B・H-5）

1. 個体調整担当が `GetCharacterRevisionQuery(character, revision)` を発行し、確定済みの安全なリビジョンから静的な設定資材や差分情報を取得します。リビジョンが存在していることだけを根拠にして、それを適用すべきであると勝手に推測してはなりません。
2. 個体調整担当は、オーナーが部品ごとに明示的に選択した更新範囲を添えて、`ProposeCharacterApplicationCandidate(companion, character, expected_character_revision, selected_parts, owner_selection)` を提案します。認証情報、権限昇格、同意の代用、特権の付与、プラグインやMCPの権限、全体共有スキル、システム制御設定、ルール、費用上限などの適用禁止種別が含まれている場合は、更新対象から確実に除外します。
3. 個体調整担当は適用関係を確定し（`AppliedAs`）、パートナーがこれまでに積んできた経験や記憶などの成長状態を決して初期化しません。また、オーナーが確認していない部品を勝手に更新済みに変えてはなりません。キャラクター資材のマスターデータ（Character担当）と、特定のパートナーへの適用関係のマスターデータ（個体調整担当）を同一のトランザクションで混同して更新してはなりません（同期区分 SD-CharApply の短いトランザクションで安全に処理します）。
4. パッケージ検証の成功を、機能の実行許可やセキュリティ認可と混同してはなりません（`PackageValidationReport` は受け入れ可否、含まれるコンテンツの範囲、信頼／非信頼の扱いを示すものであり、制御設定の変更ではありません）。
5. **失われてはならない情報**: 静的なキャラクターリビジョンと個体への適用関係の明確な分離、部品ごとのオーナーによる明示選択、パートナーの経験状態が初期化されないこと、推奨設定が存在することと実際の実行許可・有効化が別であることの認識、エクスポートデータへのプライベートな情報の混入防止。

### V-10 Task Agent result adoption と Task progress（H-A・K-H・K-K・SD-Task）

1. **正常系（current R1 → finalization → 到着 record/seal → 採用 → 完了）**: current Task が revision R1。委任 D1 は R1 を依拠して作成され（AU3 の同一トランザクションで progress は `Started → InProgress`）、D1 の execution lifetime で Action 試行 A1（Read）と A2（Create）が durable に記録されています。Task Agent の tool loop が複数 turn を実行し、final answer を提出した finalization 境界で orchestrate が `TaskResultId` X を発行し、`record_task_result_arrival(TaskAgentResultArrival { delegation: D1, result: X, body })` で本文を durable 化して同じ不分区間で D1 を seal します。その後 `TaskResultAdoptionClaim { result: X, attempt_refs: [A1, A2] }` を `adopt_result` へ渡します。1 つの `Immediate` トランザクションで、result 行の存在、依拠リビジョン R1 と現在 `task.revision`、目的 identity、`task.progress` が非 terminal であることを確認し、delegation D1 から `action_attempt` を列挙して authoritative set {A1, A2} を解決し、claim との完全一致、各行の `(task, revision, delegation)` 対応、いずれも `ConfirmedSuccess` であること、および同じ `TaskId` の全 revision / 全 delegation に `Unknown` が無いこと（Task-wide completion barrier）を確認します。同じトランザクションで `task_result_attempt` の相関行を 1 回だけ挿入し、`adopted_revision = R1` を刻み、`task.progress` を非 terminal から `Completed` へ CAS して `AdoptedAsCompletion(R1)` を返します。再起動後は `load_task` が `progress = Completed` と `adopted_result = X` を返し、`load_task_result(X)` が X → D1 → R1 → A1/A2 の対応を bounded read で辿れます。
2. **steering race（R1 結果の遅延到着）**: D1 が R1 を依拠して実行中に、owner の steering が R2 を確定します。その後に R1 の結果が到着した場合、`adopt_result` は delegation 行が依拠した revision と現在 revision の不一致を同一トランザクションで検出し、claim と authoritative set の一致を確認した上で attempt 相関だけを記録して `RecordedToOriginalOnly` を返します。R2 は完了・上書きされず、progress も変化しません（目的 identity が偶然一致していても、revision correspondence が崩れていれば現在へ採用しません）。
3. **Action Unknown（未確定の外部作用）**: 結果本文が成功を主張していても、A2（Create/Edit）の確定度が `Unknown` のままである場合、`adopt_result` は authoritative set に A2 を含めて検証し、`WithheldByEffectFacts { attempts: [A2] }` を返し、`task.progress` は `InProgress` のままです。Task が「成功して完了した」と確定されることはなく、Agent の自己申告によって `Unknown` が `ConfirmedSuccess` へ昇格することもありません。`ConfirmedFailure` の場合も同様に現在 Task を完了せず、到着 record だけを残します。同じ `TaskId` の別 delegation / 旧 revision に `Unknown` が残る場合も Task-wide completion barrier が完了を保留します（walkthrough 18）。cross-delegation の `ConfirmedFailure` だけを理由に barrier が block することはありません（walkthrough 23）。
4. **同一 result の二重 arrival**: 同じ `TaskResultId` X が再び届いても、`task_result` の PK により本文行は増えず、`task_result_attempt` も重複挿入されません。既存行の `adopted_revision` と現在状態から同じ判定を再評価するため、採用済みなら `AdoptedAsCompletion`（同じ TaskRef）を返して二度目の terminal transition を行わず、記録のみ／保留だった場合は同じ `RecordedToOriginalOnly` / `WithheldByEffectFacts` を返します。同一 identity で本文・委任・依拠リビジョンが異なる再 arrival は技術的エラーとし、既存行を上書きしません。同じ delegation に別の result identity Y の final arrival を試みる場合は AU15a が 2 つ目の final result として受理せず fail closed（walkthrough 16）。別の delegation D2 の result Y が完了後に到着した場合は `RecordedToOriginalOnly` で履歴に帰属します。
5. **terminal gate（Completed / Failed 後の新規処理拒否）**: Task R1 が `Completed`（`Failed` も同様）になった後、同じ current R1 のまま (a) `create_delegation` 要求 → AU3 が progress terminal を検出して `TaskTerminal { task, progress }` を返し、delegation は作成されない、(b) steering 要求 → AU4 が `TaskTerminal` を返し revision は前進しない、(c) Task Agent 推論開始 → AU14 の claim が terminal を検出し `TaskPremiseStale`（作業側で `TaskTerminal` へ写像）として provider I/O なし、(d) 残っている古い delegation D1 からの filesystem Action 開始 → AU5 が `TaskTerminal` を返し、`action_attempt` も外部作用も発生しない。terminal は吸収的であり、cancel marker の producer を待たずにこのスライスの比較点で拒否されます。terminal gate と execution seal gate は別概念であり、Task InProgress + delegation sealed の場合は後述の `ExecutionSealed` として拒否されます。
6. **omitted Unknown attempt（claim の部分集合）**: execution D1 の Action 試行として A1（`ConfirmedSuccess`）と A2（`Unknown`）が durable に記録されていたのに、caller が `attempt_refs: [A1]` だけを主張した場合、`adopt_result` は delegation D1 から authoritative set {A1, A2} を列挙し、claim との不一致（A2 の欠如）を技術的エラーとして fail closed します。Task は `InProgress` のままで、A2 を省略した完了は成立しません。
7. **empty attempt spoof（空集合の主張）**: execution D1 が実際に A1 を開始していたのに、caller が `attempt_refs: []` を主張した場合、authoritative set {A1} と一致しないため技術的エラーとして fail closed し、完了しません。逆に authoritative set が durable に空である場合（その execution が Action を 1 つも開始していない場合）だけ、claim: `[]` が一致して採用判定へ進めます。
8. **true no-Action result（Action 無しの採用）**: execution D1 が Action を 1 つも開始せず pure inference の final answer を提出した場合、delegation D1 からの列挙は durable に空集合になります。現在 Task/purpose が有効で terminal でなく、claim: `[]` が一致する場合、Action 無しの結果として `AdoptedAsCompletion` へ進めます（claim が空だから Action が無かった、と推定するのではなく、delegation D1 からの列挙が空であることを durable に確認します）。
9. **crash after finalization（到着 record/seal 後・採用前のクラッシュ）**: final answer を受け取り、`record_task_result_arrival` が X の本文・delegation を durable 化して D1 を seal した後、`adopt_result` の前に Host が crash した場合、再起動後に `load_task_result(X)` は本文を返し、`adopted_revision = None`（到着済み・採用未確定）を示します。provider 呼び出しは自動 replay されず、結果が自動再採用・完了されることもなく、Task は `InProgress` のままです。採用を進めるには、現在の前提を確認した上で明示的に `adopt_result` を再実行します。
10. **失われてはならない情報**: 結果 identity と本文の 1 回だけの到着 record、依拠リビジョンと委任対応（delegation = execution lifetime）、final result 到着による seal と seal 時点の authoritative set membership、Action 試行相関の authoritative set 検証結果、completion commit 時点で同じ `TaskId` の `Unknown` が 0 件であること（Task-wide completion barrier の不変条件）、採用リビジョン、現在の progress、terminal 後の新規開始拒否、terminal 後に遅延した結果の帰属、到着済み・採用未確定の状態。
11. **multi-turn + multi-action（複数 inference turn と Action の束）**: 1 delegation D1 は 1 つの delegated Task Agent execution であり、複数の inference turn を持ち得ます。T1 が A1（List）と A2（Read）を開始し、T2 が A3（Create）を開始し、T3 が final answer を提出したとします。finalization 境界で X を record すると D1 が seal され、authoritative set(X) は delegation D1 の全 Action 試行 {A1, A2, A3} です。T3 の inference attempt identity だけから列挙して `{}` や `{A3}` にはしません（source inference attempt identity と execution lifetime を混同しません）。
12. **action-before-seal race**: A1 の AU5 commit が result X の AU15a より先に成立した場合、A1 は D1 の authoritative set に含まれます。AU5 と AU15a は同一 SQLite master 上の短い `Immediate` トランザクションで直列化されるため、seal 時点の membership が一意に決まります。AU15b は {A1} を列挙し、claim と一致すれば採用判定へ進みます。
13. **action-after-seal race**: result X の AU15a が D1 を seal した後に D1 の新しい Action A2 要求が届いた場合、AU5 は同じ不分区間で seal を検出して `ExecutionSealed` を返し、`action_attempt` を挿入せず外部作用も行いません（authoritative set は {A1} のまま増えません）。
14. **inference-after-seal**: result X が D1 を seal した後、D1 の新しい inference claim は AU14 が同じ不分区間で seal を検出して `TaskPremiseStale` として拒否し、作業側の再読込（`load_delegation_result(D1)`）が `ExecutionSealed { delegation: D1 }` へ写します。provider I/O は発生しません。Task が InProgress のままであっても、同じ D1 を再利用して work を継続することはできません。
15. **in-flight Unknown（seal 前開始の未確定 Action）**: A1 が seal 前に durable start し、確定度が `Unknown` のまま result X が D1 を seal し、AU15b が `WithheldByEffectFacts(A1)` を返して Task は `InProgress` のままとします。seal は既に start した work を「無かったこと」にしないため、A1 は authoritative set に含まれたままです。その後 ene-action owner の客観的証拠が A1 を `Unknown → ConfirmedSuccess` に進めると、同じ result X の AU15b を再評価してよく、membership は seal 時点の {A1} のまま、他の条件が揃えば完了が成立します（set membership と certainty update を混同せず、membership を増減させません）。
16. **second final result（同一 delegation の 2 つ目の final result）**: D1 が result X で seal 済みのとき、異なる `TaskResultId` Y の final arrival を D1 に対して試みても、AU15a の durable invariant（1 delegation につき final `task_result` は最大 1 行）により 2 つ目の行は作られず fail closed（技術的エラー）です。同じ X の retry は本文・委任・依拠リビジョンが一致する限り冪等です。既に seal 済みの execution に新しい final result を受理しません。
17. **new delegation after sealed execution**: D1 が result X で seal されても Task が `InProgress` のまま（例: `WithheldByEffectFacts`）であれば、現在の前提・policy が許す限り新しい delegation D2 を AU3 で作成できます（Task が terminal でないため拒否されません）。D2 は新しい execution lifetime であり、D1 の Action 試行集合を引き継がず、D1 を再利用して work を継続することもありません。D2 の final result は D2 の Action 試行集合に対して検証されます。
18. **cross-delegation unresolved Unknown（reviewer reproduction）**: current Task R1。D1 の A1 が `Unknown` のまま final result X を提出して D1 が seal され、AU15b は `WithheldByEffectFacts { attempts: [A1] }` を返して Task は `InProgress` のままです。その後 D2 が AU3 で作成され、D2 の AU5 が A2 を `Unknown` として durable start します（D2 は final result をまだ持たない unsealed delegation ですが、既に開始した試行は barrier の対象です）。A1 が ene-action owner の客観的証拠で `Unknown → ConfirmedSuccess` に進んだ後に X の採用を再試行しても、AU15b は同じ短いトランザクションで Task-wide completion barrier を検証し、同じ `TaskId` に属する A2 の `Unknown` を検出するため `Completed` にしません（blockers = result-local の非成功 ∪ Task-wide の `Unknown` = `{A2}`）。Task は `InProgress` のままで、A2 の settlement を待ちます。
19. **cross-delegation Unknown settles**: 18 の続きで、A2 が `Unknown → ConfirmedSuccess` に進み、同じ `TaskId` に `Unknown` が 1 件も無くなった後に X の採用を再試行すると、他の条件（現在 revision・purpose identity・非 terminal・cancel marker 無し）が揃っていれば `Completed` が成立し得ます。`task_result_attempt(X, …)` に刻印されるのは X の result-local authoritative set（D1 の依存、例: `{A1}`）のままで、A2 を X の dependency として捏造しません。
20. **AU5-before-completion race**: D2 の AU5 が A2 を `Unknown` として先に commit した場合、後から届いた X の AU15b は同じ SQLite master 上の短い `Immediate` トランザクションで直列化され、Task-wide completion barrier が A2 を検出して `WithheldByEffectFacts` を返します。Task は `InProgress` のままです。
21. **completion-before-AU5 race**: X の AU15b が Task-wide completion barrier を通過して `Completed` を commit した後に D2 の AU5 が到着しても、`task.progress` が terminal であるため AU5 は `TaskTerminal` を返し、`action_attempt` を挿入せず外部作用も行いません（A2 の行は存在せず、完了後の外部作用も発生しません）。この 2 順序以外に「A2 が barrier に見えず、かつ terminal gate も通る」隙間は、AU5 と AU15b が同一 SQLite master 上の短い `Immediate` トランザクションで直列化されるため存在しません。
22. **settled cross-delegation success**: D2 の A2 が `ConfirmedSuccess` で settlement 済み、X の result-local 依存もすべて `ConfirmedSuccess`、同じ `TaskId` に `Unknown` が無い場合、A2 が X の dependency でなくても Task-wide completion barrier を理由に完了を block しません（`AdoptedAsCompletion` が成立し得ます）。完了 commit 後に新しい AU5 は terminal gate で拒否されるため、A2 の fact は durable に残ったままです。
23. **settled cross-delegation failure**: D2 の A2 が `ConfirmedFailure` で settlement 済み、X の result-local 依存がすべて `ConfirmedSuccess`、`Unknown` が無い場合、A2 が X の result-local dependency でなければ、unsettled-effect barrier だけを理由に完了を block しません（外部作用 certainty として settlement 済みだからです）。A2 が X の result-local set に含まれる場合だけ、既存 contract どおり `ConfirmedFailure` が X の採用を block します。
24. **old revision Unknown（steering を跨ぐ started Action）**: current Task が R1 のとき D1 の A1 が `Unknown` で durable start し、owner の steering が R2 を確定し、その後 R2 の D-new が result X を提出したとします。steering は既に開始した外部作用を取り消さないため、A1 は R2 の completion 後に外部作用を確定し得ます。AU15b の Task-wide completion barrier は `task_id` 単位（全 revision / 全 delegation）で `Unknown` を検出するため、A1 が `Unknown` の間は R2 の X を `Completed` にしません。A1 が ene-action owner の客観的証拠で settlement した後に X の採用を再試行してよく、その時点で barrier が clear なら完了が成立し得ます。barrier を current `TaskRef` の delegation に限定しないのは、この old-revision started Action を漏らさないためです。

## 18. 意図的に残した Design Freedom

- 各インターフェースの具体的なメソッド名、モジュール配置、同期／非同期の粒度（本書では `async fn` のシグネチャを提案していますが、アクターモデル、チャンネル通信、直接の関数呼び出しなどの低レベルな実装機構は固定しません）。
- 由来情報（provenance）、対象範囲（source_range）、選択前提（selection_premise）などの内部データ構造の具体的表現形式、確認ダイアログのまとめ方、権限評価エンジンの内部アルゴリズム、識別子の検証・鮮度確認・競合制御の具体的機構（CCT 第5節で示した3つの選択肢：楽観的CAS、担当者ローカルのメールボックス、短いDBトランザクション内での不可分な読み取り照合のいずれを採用しても構いません）。
- コンテキスト組み立て（Context Assembly）およびルーティング用コンテキストの生成アルゴリズム、データ形式、更新頻度、鮮度の判定基準、選択アルゴリズム、文章要約、ベクトル検索のスコアリング計算、プロンプトテンプレートの構造、キャッシュの実装方式。
- 費用予約量の具体的な計算式、集計期間、推定方式、内部リソースの配分方針、反復処理の抑制閾値、画面キャプチャのインターバル、イベント駆動とタイマー待機の具体的機構。
- ペアリングおよび初期設定における認証材料の具体的フォーマット、暗号化保護方式、受け渡しプロトコル、ホスト再起動後における元のクライアントへの再接続待機時間、ネットワーク切断の検知アルゴリズム、在席の調停手順、対話やアクションの安全な区切り処理、停止指示の伝達機構。
- 検索、無効化、検証の低レベルな実装手法、バックアップ作成時の整合時点の選定アルゴリズム、バックアップファイルの保存形式、復元時のマスター切り替え手順、暗号化アルゴリズム、検索用暗号トークン（`SealedSearchToken`）の具体的実装。
- 監査ログの出力フォーマット、診断情報およびテレメトリの収集スタック、画面提示確認のUIデザイン、未伝達要約の粒度、データの具体的な保持期間、UIの画面レイアウト。
- なお、上記の設計自由度があるからといって、システム全体を統制するような単一の巨大コンテキスト層、ポリシーエンジン、統合マネージャー、集中サービス、中央コーディネーターなどを後から無秩序に追加してはなりません。本書で定めた既存の責務分担、各領域の担当責任者（semantic owner）、ホスト／クライアント間の配置原則、および厳格な信頼境界を常に遵守しながら、具体的な実装方法を選択します。
