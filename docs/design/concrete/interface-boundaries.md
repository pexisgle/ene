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
| K-I | 外部拡張の受け入れ（MCP Tool／Resource／Prompt、限定プラグイン、サンドボックス例外） | コマンド ＋ 確定判断 ＋ 確定事実 | 機能利用元 | 実行・拡張（受け入れ・制限）＋ 権限・制約（例外の意味論） | K-9、E-4・E-5、C-5 |
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
| C-D | キャラクターのエクスポート | コマンド ＋ 確定事実 | オーナー（管理経路） | Character（静的範囲の選定・配布可能性の検証） | CH-6、CC-02 |
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

`create_task`（会話・第一者管理・自発・run now の新 Task）と `create_delegation` は、担当 Companion の canonical `Running` と削除開始の新規活動禁止 hold 不在を、Stop の `Stopped` commit / hold 確定と同じ SQLite master の短い比較・commit 点で確認します。事前に読んだ Running、会話の currentness、既存 Task の非 terminal はこの照合の代わりになりません。Stop / hold が先なら新しい Task 一式 / delegation を書かず、通常の AU2 / AU3 は `CompanionUnavailable` の domain refusal とします。AU2 / AU3 が先に commit した場合はその Task / delegation を維持し、後続 Stop は Task の Cancel と実行中処理の停止を best-effort で試みます。既存 attempt、結果、`Unknown` は消しません。予定回の Task 作成は AU18 の同じ比較・commit 点に含め、Stop の停止区間内に due となった回だけ `Missed`・Task なしとし、Stop より前に due となった未受付回は停止済みと推測せず `Unclassified`・Task なしで保護します。

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
enum TaskContextOriginKind { OwnerConversation, OwnerManagement, Spontaneous, ScheduleOccurrence }
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
    save_target: Option<WorkspaceFolderRef>,  // 作成時に Owner が選んだ保存先の写し。None は最終保存の権限なし
}

// 最終成果物の外部保存を enable するスライスで、Owner 選択の producer とともに導入する現在値。
struct OwnerConfirmedSaveTargetPremise {
    reference: SaveTargetRef,
    version: SaveTargetVersion,
    folder: WorkspaceFolderRef,
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

// タスク中断（Cancel）の要求。個体調整（会話）または第一者管理経路から作業担当へ直接届き、
// 本体LLM・Task Agent・エージェントの正常終了を待たずに即座に判定される（H-3・H-10）。
// リビジョンは問わない: 中断はタスク単位の操作であり、同時に方針指示（steering）が
// 競合しても revision stale にはしない。会話 source は対象 Task に対する明示的な撤回・置換だけを比較する。
// source は Host の認証済み入力境界が確定する。provider 出力に管理経路や Owner 意図を選ばせない。
// reason は持たない: 中断理由の本文は会話履歴・管理経路側に保持し、Task に複製しない。
struct CancelTaskCommand {
    task: TaskId,
    source: TaskCancelSource,
}
enum TaskCancelSource {
    OwnerConversation { intent: OwnerTaskCancelIntentRef }, // 会話担当が同じ master に記録した body-free の型付き正本への参照
    FirstPartyManagement { source: RawId }, // 認証済み管理操作の由来。会話の最新入力には依存しない
}

struct OwnerTaskCancelIntentRef {
    intent: RawId, // owner_task_cancel_intent.intent_id。TaskId はコマンド側と照合する
}

// 個体調整（会話）が認証済み Owner 入力を受ける時点で会話文脈・現在 Task 候補を使い、
// 直接的な中断と対象 Task を確定する。明確な「止めて」は LLM / Agent を await せず、
// History の入力受理と body-free intent を同じ master の短い transaction で記録して
// その参照を AU16 へ直接渡す。対象が曖昧なら候補に結ぶ確認待ちを記録し、Owner の
// 確認入力の受理時にだけ intent を発行する。後続入力が同じ Task の中断の撤回・置換か
// 曖昧なら元 intent / Task に PendingClarification を不可分に記録し、明示的なら
// ExplicitSupersession を記録する。確認結果は pending ID を指す新 fact として
// 撤回なら ExplicitSupersession、撤回でなければ ClarificationNotSupersession を記録する。
// 無関係な発言には相関を発行しない。
// provider output・未認証の第三者は intent / followup の producer になれない。

// 中断の確定結果（作業担当）。中断要求の受付（durable な progress 遷移）と、
// 実際の処理・外部作用の停止完了は別の事実であり、停止完了はこの outcome に含めない。
// すべて Ok 側のドメイン判定であり、技術的エラーと混同しない。
enum TaskCancelOutcome {
    CancelAccepted,                  // 現在の progress（started / in_progress）を cancelled へ不可分に遷移させた。
                                     // 「中断要求を受理した」ことだけを意味し、「停止が完了した」ことは意味しない
    AlreadyCancelled,                // 対象 Task は既に cancelled。書き込みなしの冪等な再要求（受理は 1 回だけ）
    Superseded,                      // 同じ Task の中断を明示撤回・置換した認証済み Owner 指示が後続。書き込みなし
    NeedsClarification,              // 対象・中断意図・撤回の対応が曖昧。確認まで受付保留、書き込みなし
    TaskTerminal { task: TaskId, progress: TaskProgress }, // 対象 Task が terminal（Completed / Failed）のため
                                   // 中断できない（書き込みなし。Cancelled は AlreadyCancelled として区別する）
    MissingTask { task: TaskId },    // 前提のタスクに永続状態が存在しない（書き込みなし）
}

// 作業担当による確定結果（タスク化・方針指示）。Cancel は専用のコマンド・outcome
// （CancelTaskCommand / TaskCancelOutcome）を持ち、proposal の outcome に混在させない。
enum TaskProposalOutcome {
    AcceptedAsTask(TaskRef),         // 新規タスクとして正式に受理
    AcceptedAsSteering(TaskRef),     // 新しいリビジョンとして方針変更を受理（過去リビジョンも保持）
    CompanionUnavailable,            // AU2: 担当個体の Stop / Deleted / deletion hold が先。Task 一式なし
    StalePremise { current: TaskRef }, // 期待リビジョン・依拠目的の不一致（再評価へ差し戻し）
    TaskTerminal { task: TaskId, progress: TaskProgress }, // 方針指示: 対象 Task が terminal（Completed / Failed / Cancelled）のため
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
    CompanionUnavailable,            // AU3: 担当個体の Stop / Deleted / deletion hold が先。委任なし
    StaleTaskRevision { current: TaskRef },
    TaskTerminal { task: TaskId, progress: TaskProgress }, // 対象 Task が terminal（Completed / Failed / Cancelled）のため新規委任を
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
// 1 delegation につき final result は最大 1 つ。同じ identity の retry は recorded の本文と帰属が
// 一致するときだけ冪等です。body-free 行では帰属だけを比較し、本文は再充填しません。
struct TaskResultId(/* 不透明な識別子 */);

// 結果の到着（durable な record premises）。final Task result を提出する明示的な finalization 境界でのみ呼ばれ、
// 依拠タスクリビジョンは delegation 行（委任の対応関係）が正本であり arrival は再掲しません。
// provider 出力を受け取った orchestrate が adoption より先に安全な本文または body-free
// erasure-held 到着を確定する（durable-before-visible）。credential-set revision と消去の
// 本文現在性は同じ commit で判定し、raw 本文や scrub 証明を永続化しない。
// 1 回の inference turn の provider 出力（TaskAgentTurnOutcome::Produced）は final result ではなく、
// ここへ入れて execution を seal してはなりません（intermediate output の seal 禁止）。
struct TaskAgentResultArrival {
    delegation: DelegationId,        // 委任の対応関係（依拠タスクリビジョンは行から解決）
    result: TaskResultId,            // 結果本文の identity。orchestrate が finalization 境界で発行
    body: ScrubbedText,              // credential owner が現行 revision と delegation の旧 used-version set に結合して mint
    expected_credential_set: CredentialSetRevision, // body の証明に結合した現行 revision と一致必須
}
enum TaskResultArrivalOutcome {
    Recorded { result: TaskResultId },
    ErasureHeld { result: TaskResultId }, // body-free arrival と seal は確定、採用不可
    BodyErased { result: TaskResultId }, // 同一 ID の再到着時、既存行が事後消去済み。本文を戻さない
    LegacyBodyUnverified { result: TaskResultId }, // V43→V44 で本文を除いた既存 ID の再到着。本文を戻さない
    StaleCredentialSet { current: CredentialSetRevision }, // 未確定の到着で現行 revision または旧 used-version set が変化。本文・seal とも書かない
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

// Task owner が現在の目的・残作業と客観的な達成根拠を検証した判断。caller の attempt_refs や
// final answer の文面から生成した自己申告は受け付けない。失敗試行は full set から除外しない。
// 各 non_reliance は ConfirmedFailure + RefusedBeforeEffect（確認済み無作用）に限る。
// evidence は代替達成の客観的な検証記録への参照であり、単なる成功試行の存在だけでは足りない。
struct TaskCompletionAssessment {
    non_reliance: Vec<VerifiedNonReliance>,
    achievement_evidence: Vec<RawId>,
    remaining_work: Vec<String>,
}
struct VerifiedNonReliance {
    attempt: RawId,
    reason: String,
    evidence_refs: Vec<RawId>,        // 代替達成の検証記録への参照。空なら非依拠を成立させない
}

// 採用時に検証した理由の読戻し。消去後も採用事実と body-free な相関は保持する。
struct RecordedNonReliance {
    attempt: RawId,
    reason: Option<String>,           // Recorded のみ Some。Erased は None
    reason_disposition: NonRelianceReasonDisposition,
    evidence_refs: Vec<RawId>,        // 根拠の identity のみ。参照先の本文は消去 owner に従う
}
enum NonRelianceReasonDisposition { Recorded, Erased }

// 採用判定の結果。すべて Ok 側のドメイン判定であり、技術的エラー（主張と durable 相関の不一致などの破損）と混同しません。
enum TaskResultAcceptance {
    ErasureHeld { result: TaskResultId }, // 本文なしの到着・seal、Action 相関は維持。採用不可
    BodyErased { result: TaskResultId }, // 保存後に本文消去、未採用。再採用不可（採用済みは過去事実を返す）
    LegacyBodyUnverified { result: TaskResultId }, // 移行前本文の安全性を証明できず除去済み。未採用は再採用不可
    AdoptedAsCompletion(TaskRef),    // 結果は採用と同じ不分区間で 1 回だけ確定済み（到着 record 済み）で、現在リビジョン・目的 identity と
                                     // 一致し、delegation（execution lifetime）から列挙した authoritative set が claim と完全一致し、
                                      // 成功または検証済み無作用失敗への非依拠判断が成立し、同じ TaskId の全 revision / 全 delegation に Unknown が無い
                                     // （Task-wide completion barrier）ため、同じ不分区間で Task を完了
    RecordedToOriginalOnly,          // 現在リビジョンが前進済み / Task が終端済み（progress が terminal。Cancelled を含む）。
                                     // 元の依拠リビジョンへの durable record に留め、現在 Task を変更しない
    WithheldByEffectFacts {          // claim が authoritative set と完全一致した上で、現在 Task を完了しない。
        attempts: Vec<RawId>,        // blockers = 依拠試行の Unknown / 非依拠判断を検証できない ConfirmedFailure ∪ 同じ TaskId に属する
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
// revision の前進や世代番号で lifecycle を代替してはなりません。terminal（Completed / Failed /
// Cancelled）は吸収的で、結果採用・委任作成・試行開始・cancel 受付のいずれも terminal から
// 非 terminal へ戻しません。terminal は単なる revision stale ではなく「再評価しても開始できない」ため、
// admission gate は専用の domain outcome（TaskTerminal）で拒否し、Stale* へ丸めません。
// gate は次の比較と同じ不分区間に置きます: AU3 create_delegation、AU4 forward_steering、
// AU14 Task Agent 推論試行 claim、AU5 Action 開始、AU15b 結果採用。cancel 専用の gate 条件・列・行は
// 追加しません: cancel の durable marker は progress の Cancelled そのものであり、既存の
// 「非 terminal の必須」比較が新しい委任・steering・推論 claim・Action 開始・結果採用を拒否します。
enum TaskProgress {
    Started,      // 開始: Task が正式に受理され、terminal でない限り委任可能
    InProgress,   // 進行中: 少なくとも 1 つの委任が durable に作成され、terminal 判定待ち
    Completed,    // 完了: 作業担当が採用 result と依拠 Action facts を確認して確定
    Failed,       // 失敗: 作業担当が確定した terminal failure。確認済み terminal failure の明示的な producer（`fail_task`）が
                  // 非 terminal から CAS する。閉世界の failure 分類を持ち、provider failure・NotSent・Action Unknown・Withheld な
                  // 結果・cancel を写さない。stale な依拠リビジョン・delegation は current Task を終端化できない
    Cancelled,    // 中断: 中断要求が durable に受理され（cancel_task の CAS commit）、以後の新規委任・
                  // steering・推論 claim・Action 開始・結果採用を admission gate が拒否する。この値は
                  // 「受理した」ことだけを表し、実行中の外部作用の停止完了・Unknown の解消・再実行の可否を
                  // 表さない。producer は cancel_task のみである（cancel 専用の停止フラグ・停止行を別に持たない）
}

// Execution seal（本スライス）: final result の到着（AU15a）が delegated execution（1 delegation）を seal する。
// seal は「その execution が今後新しい work を開始しない」境界であり、Task の terminal lifecycle とは別である。
// seal 済み execution は新しい inference claim（AU14）と新しい Action 開始（AU5）を拒否し、
// 既に durable start した work を「無かったこと」にしない。authoritative set（Action attempt 集合）は
// seal 時点の membership で固定され、以後 membership は増減せず、certainty だけが
// ene-action owner の新しい客観的証拠により更新され得る（set membership と certainty update を混同しない）。
// 1 delegation につき final task_result は最大 1 つ（durable invariant: UNIQUE(task_result.delegation_id) 相当）であり、
// 同じ TaskResultId の retry は recorded の本文と帰属が一致する場合だけ冪等。body-free は帰属だけを比較する。別 ID の 2 つ目の final result は受理しない。
// seal は delegation ごとの task_result 行の存在そのものであり、seal 専用の列・行を別に持たない。
```

- **開始責務と判断権限**: 個体調整担当がタスク化・方針指示（steering）・中断（Cancel）の意図を開始し、作業担当がその受理・反映・達成を確定します。`TaskProposalOutcome` や `TaskRepository` trait などの結果型は作業担当が所有し、呼び出し側（dialogue）は自身の要求型（`ProposeTaskCommand` や `ProposeSteeringCommand` 等）を作業側の値の前提（`TaskProposalPremise` や `SteeringProposalPremise`）に変換して渡します。`TaskId`・`TaskContextEntryId`・`WorkspaceAssocId` などの識別子は作業担当（`orchestrate`）が発行して `TaskCreationPremise` を構成します。方針指示の新リビジョンが記録するコンテキスト項目（採用目的・採用指示等）の `TaskContextEntryId` も作業担当が発行し、呼び出し側は依拠した現在リビジョン（`expected`）のみを渡します。タスク作成時は要求元を担当者 `AssigneeRef` として写します。権限の判断は権限・制約担当が行い、外部アクションの成功判定は実行・拡張担当が行います。
- **方針指示（steering）の結果マッピング**: 作業担当（orchestrate）は、まず `load_task` が返した現在リビジョンに対して `expected` と依拠目的を照合します（`expected != current` の場合は `StalePremise { current }`。`expected == current` であっても `premise.purpose` が現在の採用目的識別子と一致しなければ同様に `StalePremise { current }` とし、単なる目的本文の一致では照合しません）。`forward_steering` の呼び出し後は、`CommittedAs` を `AcceptedAsSteering` へ、`StaleExpected { current }` を `StalePremise { current }` へ変換し、`MissingTask`・`RevisionExhausted`・`TaskTerminal` は同じ意味・ペイロードのまま `Ok` 側のドメイン結果として返します。リポジトリのリビジョン比較照合（atomic compare）が、事前照合後の競合レースを含む現在性を包含します。`HeldByGlobalHold` は hold スライスが保留照合前提と対で追加し、このマッピングに加わります。
- **入力として必要な情報**: 採用を提案する目的本文と発生由来（作成時）、`SteeringPremiseRef`（依拠リビジョン・目的。方針指示時）、委任スコープの写し、ワークスペース利用条件、クライアント依存条件。なお、軽微な処理をタスク化せずに直接行うかどうかの判断は本体が行えますが、その閾値や分類アルゴリズムを固定化してはなりません（H-9）。
- **由来・目的・スコープの保持**: 目的本文、採用位置の識別子、発生由来、委任範囲、ワークスペースの境界情報を決して欠落させてはなりません。単なる会話上の発言記録と、タスクに実際に反映された指示内容、および未反映・保留となった内容を明確に区別します。
- **クライアント依存条件**: `client_binding` は受付時の現在条件として照合し、タスクの永続状態にはしません。実行時のクライアント条件は、後続の境界（アクション開始時など）で最新の在席世代番号や接続状態から再確認します（IB §10）。
- **期待される現在の前提条件**: 現在のタスクリビジョン、委任の有効性、ワークスペースの有効性、方針指示の前提条件。クライアント依存のタスクであれば、在席世代番号、現行接続状態、およびデバイスの可用性を照合します。
- **キャンセル（Cancel）の受理と停止完了の分離（本スライス）**: 中断要求の受理と、実際の処理遂行の停止、および外部アクションの停止完了は別個の事実です。受理は `cancel_task` の 1 つの短い `Immediate` トランザクションで、会話起点なら body-free の `OwnerTaskCancelIntent` を PK で、同じ Task / intent に結ぶ後続の型付き撤回・確認待ちを索引付き限定範囲で同じ SQLite master 上で照合し、現在の `task.progress` が `Started` / `InProgress` のときだけ `Cancelled` へ CAS します。この操作は identity を発行せず、会話 source と同じ Task への明示撤回・置換の照合も同じ commit に閉じるため、`orchestrate_cancel` のような別ラッパーを設けず `cancel_task` 自体が H-A の受理境界です。この commit が受理の唯一の durable な事実であり、メモリ上の cancellation token・`Notify`・Future drop・停止シグナルの送信は受理の権威ではありません（再起動や別プロセスでは失われ得ます）。受理は停止完了を待たず、停止完了を受理の条件にもしません。`Cancelled` は terminal（吸収的）であり、既存の admission gate（非 terminal の必須）がそのまま新しい委任 AU3・steering AU4・推論試行 claim AU14・Action 開始 AU5・結果採用 AU15b を拒否します（cancel 受理・停止状態の専用列・行・フラグ・gate 条件を追加せず（Group B の Owner intent / followup は会話 source の帰属事実）、cancel の結果型は専用の `TaskCancelOutcome` 1 つに閉じ、AU3/AU4/AU14/AU5/AU15b の outcome へ cancel 専用バリアントを足しません。`task_result` 行の存在が execution seal であるのと同じく、状態そのものが marker です）。cancel が禁じるのは新規 work の admission（AU3/AU4/AU14/AU5 の新規開始）と現在 Task への採用・lifecycle 前進（AU15b の `adopted_revision` 刻印と terminal CAS）であり、already-started activity の事実記録（AU15a の到着 record と execution seal、AU15b の検証済み `task_result_attempt` 相関、Action の確定度 CAS、利用実績・未伝達登録などの事実記録）は cancel によって禁止しません。同じ Task の中断を明示撤回・置換した認証済み Owner 指示が先に commit した会話起点要求だけを `Superseded`（書き込みなし）とし、対象・意図・撤回が曖昧なら `NeedsClarification` で確認まで受付保留します。無関係な後続入力・別 Task の指示では中断要求を失効させず、受理済み cancel を取り消しません。第一者管理経路は認証済み操作 source / 対象 Task を同じ点で照合し、会話の現在性を要求しません。`Completed` / `Failed` の Task は中断できず `TaskTerminal`、既に `Cancelled` の Task は `AlreadyCancelled` を返し、どちらも書き込みません（受理は 1 回だけ）。`Cancelled` は吸収的であり、中断解除・progress の巻き戻し・同じ Task の delegation の再利用によって再開しません。再実行はユーザーの指示の下で新しい Task を作成し、その Task の下に新しい delegation を作成して行います。中断は revision を問わず、同時 steering と競合しても revision stale にはせず、会話 source の帰属と同じ Task への後続の明示撤回・置換がない限り勝った順に durable に残します（steering が先なら revision 前進は取り消さず、その上で中断を記録します）。開始済みの推論試行・Action 試行には遡及せず、既に durable な試行行（`inference_attempt` と `data_use` 相関、`action_attempt`）を削除・無効化・書き換えません。実行中の provider I/O や外部作用の停止はベストエフォートであり、停止できなかったこと・既に作用したこと・成否不明（`Unknown`）はそのまま維持して事実として報告します（`Unknown` を cancel で成功・失敗・未実行へ書き換えません）。停止のための新規アクションを開始しません。遅延して届いた final result は AU15a が消去照合を通過した本文、または body-free erasure-held の identity / 到着事実を 1 回だけ記録し、 execution seal を行い（cancel は到着 record を握りつぶさない）、AU15b は `task.progress` が terminal（`Cancelled` を含む）であるため attempt 相関の記録に留めて `RecordedToOriginalOnly` を返し、現在 Task を完了・採用しません。再起動後も `Cancelled` は durable に読めます。再起動や再ロードで失われるのはローカルの Future・cancellation token・停止 handle・ベストエフォート停止の進捗だけであり、開始済みの provider 処理や外部作用が停止したとは推定しません（durable な `inference_attempt`・`action_attempt`・`Unknown`・遅延結果の帰属はそのまま維持します）。新しい work を開始せず、自動再開・自動 replay・自動再採用を行いません。停止完了を表す追加の durable 状態（stopped フラグ、stop 行、cancel 専用の完了状態）は置かず、何が止まり何が不明かは Action 試行の確定度と推論試行の durable facts から報告します。タスク中断のコマンドは理由本文を持たず、管理経路や会話履歴側の記録から報告します（監査・管理操作の producer が存在するスライスが起源参照を追加します）。
- **結果の到着 record と identity・本文・相関（本スライス）**: 許可された結果本文の master は Task owner（作業担当）が `task_result` の 1 行として保持（消去 held は body-free）し、identity は Task Agent execution が final result を提出した明示的な finalization 境界で作業担当の orchestrate が発行する `TaskResultId` です。orchestrate は final result の可視化より前に `record_task_result_arrival` で `TaskAgentResultArrival { delegation, result, body: ScrubbedText, expected_credential_set: CredentialSetRevision }` の scrub 済み本文または body-free held を 1 回だけ durable に記録します（durable-before-visible。同じ identity の再 arrival は本文行を増やさず、委任・依拠リビジョンの不一致と本文が残る `recorded` での本文不一致は技術的エラー。同じ本文なら既存 disposition を返し、`ErasureHeld` / `BodyErased` は帰属だけを比較して本文を復元しない）。provider 出力・Agent の自己申告・結果本文の到着は、外部作用の成功・Task の採用・Task の完了のいずれの証拠でもありません。1 回の inference turn の provider 出力（`TaskAgentTurnOutcome::Produced`）は Action 要求や途中経過であり得るため final result ではなく、この到着 record に入れて execution を seal してはなりません。`adopt_result` は同じ短い `Immediate` トランザクションで、result 行の delegation（1 delegated Task Agent execution の lifetime。0..N inference turn / 0..N Action attempt / 0..1 final result）から Action owner の `action_attempt` を列挙して authoritative set（その execution が durable start した Action 試行の完全な集合）を解決し、claim の `attempt_refs` との完全一致（欠如・追加・重複のいずれも不可）を要求します。一致しない claim は完了材料にできず、不整合な単位として技術的エラー（fail closed）です（推測で stale や `WithheldByEffectFacts` に丸めません）。Stage 6 C3 では、到着実引数の sealed `ScrubbedText` と `CredentialSetRevision` の一致を検証し、同じ `Immediate` トランザクションで現在の credential-set revision と delegation の旧 used-version authoritative set を証明と照合してから本文受入・seal を行います。stale な premise は `StaleCredentialSet { current }` として本文を一切書かず（execution seal も刻まず）、実行側は外部作用を再実行せずに元の final answer を現在値と全旧 lease 値で再 scrub してから再提出します（bounded。上限到達は domain refusal であり、古い本文や raw 本文を保存しません）。
- **結果の確信度と採用判定**: エージェントの自己申告を外部作用の成功証拠として過信してはなりません。外部作用の確信度は実行・拡張担当が記録した確定事実（fact）を参照・集約し、作業担当側で勝手に更新してはなりません。claim が authoritative set と完全一致した上で、result-local の `Unknown` は必ず完了を阻止します。確認済み無作用の失敗（`ConfirmedFailure` + `RefusedBeforeEffect`）だけは Task owner が代替達成・残作業なし・客観的根拠を検証し、試行ごとの非依拠理由と根拠参照を採用と不可分に durable 記録するなら許容します。部分的作用や根拠不足の失敗は到着 record と result-local 相関だけを残して保留します。これに加えて、`Completed` への CAS と同じ短い不分区間で **Task-wide completion barrier**（同じ `TaskId` に属する全 durable `ActionAttempt`（全 revision・全 delegation。steering 前の古い revision の started Action を含む）のうち `certainty = Unknown` の集合が空であること）を検証します。1 件でも `Unknown` が残れば現在 Task を完了せず、`WithheldByEffectFacts { attempts: blockers }`（blockers = authoritative set の `Unknown` / 非依拠判断が成立しない失敗 ∪ Task-wide の `Unknown`。集合として重複を 1 回に畳む）を返します。barrier の入力は `action_attempt` の durable facts（`task_id`・`task_revision`・`delegation_id`・`certainty`）だけで、timestamps・delegation liveness・ephemeral agent alive flag・結果本文・自己申告・caller の `attempt_refs` は使いません。result-local authoritative set は「この result が何に依拠したか」を証明する相関であり、`task_result_attempt` に刻印するのはその検証済み集合だけです。barrier が見つけた別 delegation / 旧 revision の試行は `task_result_attempt` へ追加しません（その Action が result の dependency であるとは限らないため）。cross-delegation / 旧 revision の `ConfirmedSuccess` / `ConfirmedFailure` は barrier だけを理由に完了を block しません（result-local の失敗は確認済み無作用に対する検証済み非依拠判断が無い場合に block します）。in-flight の inference attempt と、final result を持たない unsealed delegation の存在は barrier に含めません（started Action が無い、または started Action がすべて settlement 済みなら完了を block しません。完了 commit 後は terminal gate が新しい AU14/AU5 を拒否します）。authoritative set が durable に空である場合（その execution が Action 試行を 1 つも開始していない場合）に限り、Action 無しの結果として採用判定へ進めます（claim が空だから Action が無かった、と推定してはなりません）。`Unknown` は新しい客観的証拠が実行・拡張担当側で確定するまで保持します。authoritative set の membership は final result の到着（seal）時点で固定され、seal 後に AU14/AU5 が新しい開始を拒否するため増減しません。seal 後に `Unknown` が客観的証拠で `ConfirmedSuccess` へ進展した場合、同じ result の採用判定を再評価してよく、membership は seal 時点のままです（set membership と certainty update を混同しません）。
- **永続化とコミット**: タスクの新規作成は、`task + task_revision + 初期 task_context_entry` と、関連付けを確定した場合の `workspace_assoc` を不可分に永続化してから外部へ可視化します（durable-before-visible）。方針変更は、新リビジョンと新コンテキストを不可分に進めます。結果の到着 record は、`task_result` の 1 回だけの insert（安全な `body` 1 行または body-free `erasure_held`、`adopted_revision = NULL`、`task_id`/`task_revision` は delegation 行から写す）として final result の可視化前に確定し、同じ不分区間でその delegation を seal します（durable invariant: 1 delegation につき final `task_result` は最大 1 行。同じ `TaskResultId` の retry は帰属と、recorded なら本文が一致する場合だけ冪等（不一致は技術的エラー。body-free は帰属だけを比較）、同じ delegation の異なる `TaskResultId` は 2 つ目の final result として受理せず fail closed）。採用と完了確定は別の短い `Immediate` トランザクションで、result 行の存在と recorded disposition の確認、delegation（execution lifetime）からの authoritative set 列挙と claim の完全一致、確定度の読み取り、Task-wide completion barrier（同じ `TaskId` に属する全 revision / 全 delegation の Action 試行に `Unknown` が無いこと）の検証、`task_result.adopted_revision` の刻印、`task_result_attempt` の相関刻印、`task.progress` の terminal への CAS を行います。現在リビジョン不一致・`task.progress` が terminal（`Completed` / `Failed` / `Cancelled`）の場合は、到着 record に留めて現在 Task を変更しません。また、`create_delegation`・`forward_steering`・推論試行 claim（AU14）・Action 開始（AU5）はそれぞれの同一トランザクションで `task.progress` の非 terminal を必須とし、AU14/AU5 はさらに delegation が seal 済みでないこと（その delegation の `task_result` 行が存在しないこと）を同じ不分区間で必須とします。terminal は書き込み・開始ともに行いません。同一 `TaskId` に対する方針指示、委任受付、完了確定、結果採用は、同期区分 SD-Task によって厳格に逐次化します（CCT §4）。
- **Task progress / lifecycle**: Task revision（目的・指示の steering）と `TaskProgress` は別の軸です。`create_task` は `Started` で初期化し、`create_delegation` は `Started → InProgress` を同じ AU3 トランザクションで進め、`adopt_result` は採用成立時に、同じ不分区間で Task-wide completion barrier（同じ `TaskId` に属する全 revision / 全 delegation の Action 試行に `Unknown` が無いこと）を検証した上で `progress` を非 terminal から `Completed` へ CASします。`cancel_task` は会話起点なら `OwnerTaskCancelIntent` と元 intent / Task に結ぶ型付き後続 fact を同じ不分区間で照合し、`Started` / `InProgress` から `Cancelled` へ CASします（AU16）。terminal 状態は吸収的で、terminal からの遷移は `RecordedToOriginalOnly`（結果）に留まります。`Failed` は作業担当が確定した terminal failure のみを表し、provider の一時障害・`NotSent`・Action `Unknown`・`WithheldByEffectFacts`・cancel を写しません（`fail_task` は明示的な確認済み terminal failure の producer であり、それらの typed outcome から自動変換しません）。start 要求から結果が返るまでの推論ターン自体は lifecycle を変更しません。**terminal 後の新規委任・steering・推論試行 claim・Action 開始・結果採用の拒否（admission gate）は AU3/AU4/AU14/AU5/AU15b の比較と同じ不分区間で `task.progress` が非 terminal であることを必須とし**、terminal は専用の domain outcome（`TaskTerminal`、結果採用は `RecordedToOriginalOnly`）で書き込み・開始ともに行いません。cancel はこの terminal 集合に `Cancelled` を追加するだけで、cancel 専用の gate 条件・列を追加しません。execution seal はこれとは別の gate であり、Task が non-terminal（InProgress）のままでも seal 済み delegation の AU14/AU5 は拒否されます（`ExecutionSealed`）。逆に Task が terminal なら seal の有無に関わらず `TaskTerminal` です。seal は Task の terminal への遷移を意味せず、Task terminal は execution seal の前提でもありません。
- **会話 production 経路の Task control protocol（本スライス）**: 個体調整（会話）の provider 出力は Task authority そのものではありません。companion は自分の provider 出力の**最初の non-empty line**に置かれた閉世界の task-control directive（`[task-control]` に続く JSON）を解釈し、composition root（Host）の port が既存の Task owner 操作（AU2 作成・AU3 委任・AU4 steering・AU16 cancel・AU17 resume・report 読み出し）へ写します。`[task-control]` は予約された内部 protocol であり、最初の non-empty line 以外（prose の途中、複数回、trailing prose 付き、malformed）に現れた場合は invalid として fail closed します: Task operation を実行せず、返信も保存せず、stream を Interrupted で閉じます。したがって marker と directive JSON はどの位置でも presentation / History に現れません。protocol は field set まで閉じており（各 command ごとに exact な key のみ。未知 field は invalid）、意味判断・SQL は dialogue にありません。会話 Cancel は上位 CA / action-execution §7.1 の Owner からの即時受付を優先し、LLM・Agent の応答やこの directive を待ちません。LLM 経由の遅延・重複 `cancel` directive も元の認証済み Owner 指示への帰属と Task 対応を AU16 で再照合し、provider 出力だけで中断意図や対象を mint しません。task-control turn では provider の自然文を reply として残さず、owner の typed outcome だけを **scrub した後** に presentation し、同じ文字列を History に保存します（presentation と durable reply は一致）。ordinary conversation は最初の non-empty line が marker でないと判明した後も rolling marker detection（marker prefix の可能性がある末尾だけを保持）で marker を検出し、ordinary live streaming を維持します。technical failure は stream を Interrupted で閉じて未確定の成功表現を final として表示しません。
  - **Workspace authority**: Task の Workspace association は Owner / first-party premise からのみ成立します。第一者管理経路（`(SelectWorkspace, "workspace:{path}")`）で Owner が選択し、Host が canonical directory として検証した folder だけが `WorkspaceNeedRef` となり、provider / model 出力の任意 path は association・delegation scope・Action authority のいずれにもなりません（protocol 自体が workspace field を持たず、extra field は invalid として拒否されます）。
  - **作業範囲と成果物の保存先**: `WorkspaceNeedRef.folder` は Task が参照・作業できる範囲、`save_target` は作成時に Owner が別途確認した最終成果物の保存先の写しです。前者の選択から後者を推論せず、`None` を Workspace root や `report.md` 等の既定保存先へ自動補完しません。作成時の `workspace_assoc` は不変の snapshot として保持します。Task Agent に最終成果物の外部保存を enable するスライスは、作成時に選ばれた保存先を初期値とし、後からの Owner 選択も受け付ける Task ごとの独立した現在保存先 ref / 版と producer を同時に追加します。`OwnerConfirmedSaveTargetPremise` はその durable な Owner 確認事実へ解決される ref・版と検証済み folder の組であり、caller の任意 path から mint できません。producer は認証済み Owner の選択と保存先の実体・範囲を検証して CAS で現在値を確定し、Task revision や既存 `workspace_assoc` を書き換えません。producer が無い間は保存先の写しを生きた許可として使わず、`save_target = None` の Task の最終成果物は外部ファイルに保存できず、Owner 確認待ちです。
  - **Owner currentness と commit の linearization**: 会話起点の Task 作成・steering・resume は、依拠した Owner message が同じ SQLite master 上でまだ最新の accepted Owner input であることを Task 書き込みと同じ短い不分区間で比較します。新しい Owner input が先なら旧 turn の directive は `Superseded`（書き込みなし）で、reply も adopted されません。**Cancel だけ**は `CancelTaskCommand::source` の `OwnerConversation { intent: OwnerTaskCancelIntentRef }` を AU16 へ渡し、同じ master の body-free intent（Owner message ID・accepted sequence・Owner role・Companion・対象 Task・直接的中断意図）を PK で照合し、元 intent / Task に結ばれた後続の型付き `ExplicitSupersession` があれば `Superseded`、未解決の `PendingClarification` があれば `NeedsClarification` とします。無関係な後続入力や別 Task の発話は撤回ではなく、当初の対象・意図が曖昧なら会話担当が intent を発行せず Owner 確認へ進め、後続の撤回が曖昧なら `PendingClarification` に基づき `NeedsClarification` とします。モデルの意味分類の推測だけで直接性・撤回を保証せず、遅延 directive・重複は元の Owner 指示への帰属を照合し、モデル自身や未認証の第三者の発言から cancel を発行しません。管理経路の `FirstPartyManagement { source }` は認証済み操作と対象 Task を同じ比較点で照合し、会話の最新入力比較を課しません。reply append の `expected_owner_message` は従来どおり保ち、Cancel の Task CAS と reply publication を混同しません。
  - Cancel の accepted sequence は認証済み Owner 入力受理と不可分に body-free intent / followup へ刻む受理順序です。AU16 は会話担当が確定した Task と直接的中断意図を intent PK で、同じ intent / Task に結ぶ後続の型付き撤回・未解決確認待ちを索引付き存在 probe で読みます。欠損・帰属不整合は技術的エラーとして閉じ、History 本文の走査・自然文の再推測・単なる「最新発話」の比較を行いません。
  - **実行開始と steering 後の実行**: Task 作成 + delegation が受理された後、serving process（`conn::run` が所有する shared `Arc<HostHandle>` と provider transport）が install した launcher が既存 `run_task_agent` を background 起動します。会話 turn は execution の完了を await せず、runner の technical failure は `TaskProgress::Failed` に写像しません。会話からの steering が `AcceptedAsSteering(new_ref)` を確定した場合も同様に、AU3 で new revision 用の**新しい delegation** を作成し（old delegation の再利用・replay をしない）、conversation projection を新 delegation へ更新して launcher で起動します。AU3 の前にさらに revision が進んだ場合は既存の stale outcome を尊重し、old execution は既存 revision gate により次の admission で自然に stale / refused となって停止します（cancel 専用 stop state は追加しません）。会話 projection と trusted Workspace selection は in-memory の convenience です。再起動・再接続後の選択と再開は H-A.1 / IPC §18.2 に従います。
- **Task Agent の推論開始（K-E との接続）**: 委任された一時エージェントの推論は、依拠タスクリビジョンの前提を推論試行の確定（attempt claim）と同一の短いトランザクションで照合してから開始します（照合条件は K-E。現在の `task.progress` が非 terminal であることに加え、その delegation が seal 済みでないこと（その delegation の `task_result` 行が存在しないこと）を含みます）。照合に失敗した場合は `TaskPremiseStale`（タスク前提の不一致。terminal と execution seal を含む）または `Stale`（同意・認証情報の不一致）として送信前に不受理とし、provider への I/O を行いません。現在の消去条件が送信する論理入力の source 相関を覆う場合は `DataUseHeld`（data-use hold）として区別し、`TaskPremiseStale` / `Stale` へ丸めません（後述の Stage 4 erasure-currentness foundation が、canonical current-condition store の照会として claim と同じ不分区間で判定します）。作業側は再読込で delegation の final result を検出した場合、`StaleTaskRevision` ではなく `ExecutionSealed { delegation }` として返し、progress の terminal を検出した場合は `TaskTerminal { task, progress }` として返します。確定済みの試行は、その後の方針指示（steering）によって開始を取り消されません（遅延結果は元リビジョンへの記録に留め、現在のタスクへ自動採用しません）。委任レコードの存在を実行中・生存の証拠にしてはなりません（CI §5.3）。
- **Task Agent execution の一度だけの開始（one-shot、本スライス）**: 1 delegation = 1 delegated Task Agent execution の lifetime は、durable に開始した後は同じ delegation として再開しません。最初に commit した durable attempt（AU14 の推論 claim または AU5 の Action 開始）がその execution の start marker であり、`delegation_has_started_work` が両 attempt 表の delegation 相関を bounded に引いて start marker の有無を返します（`inference_attempt(delegation_id)` の V23 query-support index と `action_attempt(delegation_id)` の index が probe を支え、index は意味状態ではありません）。停止した unsealed execution、ローカルの running registration を失った execution、再起動を跨いだ execution のいずれも、start marker がある限り新しい run として開始せず（`ExecutionAlreadyStarted`）、継続は新しい delegation（新しい execution lifetime）として行います。同一プロセス内の同一 delegation への二重 run は、per-delegation の running registration が挿入と同一の lock 区間で拒否します（`ExecutionAlreadyRunning`。local registry は canonical state ではなく、その消失や非登録を停止・生存・開始可否の証拠にしません）。開始前 probe は、Task が terminal・execution が seal 済み・依拠リビジョンが前進済みの場合は各 owner の outcome（`TaskTerminal` / `ExecutionSealed` / `StaleTaskRevision`）を先に成立させ、start marker の拒否はそれ以外の開始可能な run にだけ適用します。Stage 5 の production launcher はさらに、CCT §7.4 の当該プロセスで commit と対になった launch 予約を必須にし、start marker のない古い delegation も ID 指定だけでは起動しません。両 refusal は provider I/O も Action も開始しない開始前の domain outcome であり、Task progress・attempt 行・execution seal・`Unknown` を変更しません。
- **Task Agent の入力・採用指示本文の解決・scrub（採用指示本文の契約）**: 推論ターンの論理入力は、依拠リビジョンの採用目的本文（`task_revision` snapshot）に、現在リビジョンまでの `AdoptedInstruction` 項目の本文を `TaskContextEntry` の順序のまま並べたものです。採用指示本文の正本は `origin.kind` に応じた History の `history_message` 行、または第一者管理操作の `activity_record` 行（いずれも `origin.source` が示す）であり、Task 側（`task_context_entry`・delegation・inference attempt・task_result）へ本文を複製してはなりません。本文は作業側が定義する `TaskInstructionSource` port で解決し、Host 結合ルート（`apps/ene-core`）が個体調整 owner の単一レコード bounded read（History は `message_id`、管理操作は `activity_id` の PK を直接引く。timeline 全読込・recent timeline・command lookup は代用しない）へ写します。ハーネスは採用指示項目ごとに (1) `origin.kind` が `OwnerConversation` または `OwnerManagement` で読み戻しの kind と一致すること、(2) 読み戻したレコードの `source` が `origin.source` と一致すること、(3) `role = Owner` であること、(4) `companion` が `task.assignee.companion` と一致することを検証し、不一致・未対応 kind・読み出し失敗は fail closed の技術的失敗として provider I/O を開始しません（別 Companion の Owner 発言を混入させません）。参照先が存在しない場合は `TaskAgentTurnOutcome::InstructionSourceMissing { entry, source }` として、本文を捏造せず・黙ってスキップせず・`TaskContextEntry` を削除／書き換えずに turn を終えます。目的本文と解決済み指示本文は 1 つの論理入力へ組み立て、`SecretScrubber` を 1 回だけ通します。採用指示本文（および採用目的本文）を provider へ送る経路は、後述の Stage 4 erasure-currentness foundation（canonical current-condition store の照会を AU14 claim と同一トランザクションへ統合）が実装された後の契約です（本文の bounded read・検証・論理入力組み立て・scrub は foundation に先行して実装できますが、provider-send-enabled として foundation より先に merge しません）。ハーネス自ら `ScrubbedText` を構成せず、複数回 scrub した断片を合成しません（構成の封止は Issue #1530 で追跡）。ワークスペース範囲そのもの（フォルダ一覧やファイル列挙）はこの推論経路から送信しません。実行スライス（D）の tool loop が行う Action の往復は、実行ローカルの transcript（provider 出力と Action owner の観測）として同じ組み立て・単一 scrub の枠内に加わり、Task 側へ複製・永続化せず、canonical source 相関（`data_use`）にも加えません。ただし送信本文の mechanical coverage には必ず含めます。どの provider 出力を final とするかの低レベル protocol と prompt 文言は実装の自由度であり（後述）、model 出力が要求する操作は必ず AU5 と権限・workspace 境界を通ります。scrub の失敗は fail closed の技術的失敗であり、原文を送信・保存・ログ出力しません。Task Agent は `admit_task_agent` のみを呼び、`admit_dialogue` / `admit_learning` を代用してはなりません。
- **Task Agent 送信の data-use premise と linearization point（Targeted Deletion currentness）**: 論理入力の本文を解決した後も、ハーネスは「どの canonical source から取り込んだか」を失ってはなりません。採用目的 entry と全採用指示 entry の `origin.source` を context 順に並べ、H-A.1 の既実行 facts ブロックに使った ActionAttempt / TaskResult の canonical source identity を続けた `Vec<RawId>`（`data_use`。本文でも hash でもない）として `TaskAgentInferencePremise` に渡し、Host 結合ルートが推論側 `TaskAgentAttemptPremise` へ写して attempt claim と同じ短い `Immediate` トランザクションで durable に記録します。同じ source が複数 entry から採用されていても entry ごとの相関を保持し、source ID で entry identity を dedupe しません（gate は相関の集合として coverage を判定してよく、そのために prompt 本文を cache・retry 用に保存しません）。本文そのものを premise・attempt 行・消去条件・ログ・監査へ複製せず、source identity を hash で置き換えません。**History の bounded read が行を返したこと（`load_owner_instruction` の成功）も `SecretScrubber` の成功も、送信 authority ではありません**。行が存在することと、その本文を今 provider へ送信してよいことは別事実であり、送信の第一の admission は claim commit が与えますが、最終 dispatch 時にも current condition を再照合します。scrub の成功は credential-like secrets が scrub されたことだけを意味し、消去条件の currentness を意味しません。**linearization point**: provider I/O を開始してよいのは、Task Agent 推論試行 claim（AU14）が成功した後のみです。claim は既存の 5 条件（delegation 行の存在、依拠 `TaskRef` 一致、現在 `task.revision` 一致、`task.progress` 非 terminal、delegation 未 seal）と同意・認証情報 premise に加えて、`data_use` の各 source 相関と送信する論理本文全体が現在の durable な消去条件（保全・消去 owner の `DeletionOperationId` + `DeletionSweepGeneration` + `ErasureConditionRef` に相当する現在条件。source 相関を機械的に照合できる形で enforcement より先に durable 化）に覆われていないことを同じ不分区間で照合します。この canonical current-condition store は Stage 4 erasure-currentness foundation が導入し、Stage 6 の Targeted Deletion producer が同じ store へ durable-before-enforce で active condition を投入します（別 store / 別 gate / 別 source correlation を作りません）。store の active condition set が空である場合は、placeholder ではなく store を実際に照会した authoritative な「被覆なし」として current coverage を満たします（完了後の旧由来バリアは別途必須）。claim と消去条件の durable 化は同一 SQLite master 上の短い `Immediate` トランザクションで直列化し、provider I/O を長い transaction・global lock・Task-wide mutex で囲みません（CCT §15.2）。**成立する 2 順序のみ**: `消去条件が durable → claim が coverage を検出` なら claim は送信を拒否し provider は 0 バイトも受け取りません。`claim commit → dispatch 再照合 → provider I/O 開始` なら attempt 行（と `data_use` 相関）が already-started use として durable に残り、その後の deletion 開始に対しては削除参加の列挙対象（ベストエフォート停止・縮小、遅延結果の再保存防止）になります。本文を claim 前に materialize 済みでも、この claim 順序に加えて dispatch と削除開始を直列化し、claim 後の deletion が勝てば送信前に再照合して拒否します。
- **deletion refusal の表現と materialized prompt の扱い**: 消去条件による refusal は Ok 側の domain outcome とし、`NotSent` の data-use hold 理由（推論側 `NotSentReason::DataUseHeld` と作業側 mirror `TaskAgentNotSent::DataUseHeld`。Stage 4 erasure-currentness foundation が claim 内の data-use 照合と対で追加）で表します。`StaleTaskRevision` / `TaskTerminal` / `ExecutionSealed` / `InstructionSourceMissing` / `InputUnavailable` / `TaskAgentInferenceError`（技術的エラー）へ意味を偽装せず、storage エラーとして扱いません。`DataUseHeld` は deletion 完了後に解除されて送信可能になるものではなく、その source の送信自体が現在禁止されていることを表します。解決済み本文と scrub 済み prompt はその turn のメモリ上にのみ置き、cache・retry・ログ・監査・別 record へ保存しません。claim 拒否時は materialize 済み buffer と解決済み本文を破棄し、provider I/O を開始せず、「History 行がもう無いから追跡不能」とせず `data_use` の source 相関を refusal の説明に用います。claim が先に確定した場合の遅延 provider 結果は、既存の削除契約（受入境界での currentness 照合、現在 Task への自動採用禁止、body-free `ErasureHeld`（対象本文を含む場合）の帰属、確定度の非改ざん）に従わせます。
- **purpose text との対称性**: この data-use premise は採用指示専用ではなく論理入力全体（採用目的 entry の `origin.source` と全採用指示 entry の `origin.source`）を対象にします。採用目的本文の正本は Task 側 `task_revision` snapshot であるため、その master の消去は本文だけの in-place 書換えで済ませず、AU14 が既に比較する task 側 premise（task 行の存在・`task.revision`・`task.progress`）を不成立にする形で enforcement します（instruction だけ gate し purpose は race 可能、という非対称を作りません）。
- **enable 条件と Stage 4 / Stage 6 の責務分離（producer-less placeholder の禁止）**: 実装順序は Stage 4（Task Agent + filesystem task）→ Stage 5 → Stage 6（Targeted Deletion）であり、Stage 4 の完了は Task Agent が実際に Workspace を読み、provider send を含む推論 turn を完了できることを含みます。したがって Stage 4 に先行導入するのは Targeted Deletion 全体ではなく、送信 currentness を判定するための最小基盤（**Stage 4 erasure-currentness foundation**）だけです。foundation が持つのは、(1) 保全・消去 owner の canonical current erasure-condition state、(2) その durable store（restart 後も現在条件を読める persistence）、(3) `data_use` の source 相関に対する coverage 判定、(4) AU14 claim と同一の不分区間での currentness compare、(5) `DataUseHeld`、(6) inference attempt の `data_use` durable 相関、だけです。ユーザー向け Targeted Deletion の request intake、目的確認、対象の意味解析、機械的検索・検索トークン、`DeletionOperation` lifecycle、deletion sweep orchestration、participant の列挙・調整、実際の局所消去、遅延結果収集、残存検証、finalizing、全域完了、監査・完了報告、UI / 管理画面、バックアップ統合は Stage 6 に残します（Stage 4 へ前倒ししません）。
  Stage 4 時点ではユーザー向け deletion operation を開始する producer がまだ存在しないため、canonical store の active covering conditions は通常 `{}` です。これは `ErasureConditionRef::none()` / `DeletionSweepGeneration(0)` / 「削除なし」marker / always-current のような synthetic な placeholder とは異なり、**canonical durable store を実際に照会した結果としての authoritative empty set** です。Stage 4 erasure-currentness foundation が実装済みであり、AU14 が canonical active-condition set を同じ claim transaction で確認できる限り、active set が空でも provider send を enable します（full Targeted Deletion producer は不要です）。逆に、store の照会を欠いたまま `NoDeletion` 相当の既定値や派生 marker で送信経路を先行 enable すること、producer を先に有効化して gate 無しで消去を enforcement することは禁止します。
  `#1531` 由来の既存 purpose-only Task Agent send は、foundation 導入前の現行 production path です。削除開始 producer 自体がまだ存在しないため現時点で deletion start との race は実際には発生せず、この設計は既存送信を retrospective に security violation として無効化するものではありません。ただし adopted-instruction send を provider-send-enabled にする前に foundation を導入し、Stage 4 の最終状態では purpose-only の特例バイパスを残さず、論理入力全体（purpose + instructions）を同じ gate へ統合します。本文の bounded read・source/role/companion 検証・`InstructionSourceMissing`・論理入力組み立て・scrub は foundation に先行して実装してよく、その場合も本文を含む prompt を provider へ渡す経路は有効化しません（未実装を fake gate や placeholder の成功で隠さず、明示的な未送信の扱いにします）。必要なら Stage 4 foundation は既存 Group J `erasure_condition` persistence の最小部分を先行実装してよく、新しい temporary table / compatibility table を作りません。保全・消去 owner の production crate がまだ存在しない場合は、後続の Stage 4 implementation slice が最小 crate / contract を導入してよいものとし、backup / restore / retention / full deletion orchestration / audit manager を同時に実装しません。
- **依存性の反転（Inversion）**: 保全・消去等の他ドメインのデータ（`HoldConditionRef` や `RestoreGeneration` 等）は、クレート境界では受け入れ側のドメイン（作業、推論、実行等）が定義する前提型として受け取ります。他ドメインの固有型を直接インポートしてはなりません（CM §4.3）。
- **IPC 通信**: ホスト内部で完結します。クライアントへは進捗や結果の必要な表示情報のみを伝達し、タスクのマスターデータそのものは送信しません。

```rust
// Task Agent の推論ターン（作業側が定義する port。Host 結合ルート apps/ene-core が
// InferenceExecutor へのアダプターとして実装する。作業側は権限・認証情報の具象型を参照しない）。
struct TaskAgentTurnPremise {
    delegation: DelegationId,        // 実行する委任。行の存在は生存の証明ではない
    exchanges: Vec<TaskAgentActionExchange>, // この execution が既に行った tool 往復（古い順）。
                                     // 実行ローカルで永続化せず、正本は Action owner の観測のまま。
}
// 1 回の tool 往復。request は Action を要求した provider 出力、observation は実行側が
// 観測した結果（または拒否クラス）。どちらも Debug では伏字化し、次の turn の論理入力へ
// replay するためだけにメモリ上へ保持します（task_result 以外の durable 本文を作りません）。
struct TaskAgentActionExchange {
    request: TaskAgentOutput,        // provider 出力。final Task result でも seal でもない
    observation: TaskAgentObservation, // 実行ローカルの観測。canonical source ではないが送信本文 gate の対象
}
struct TaskAgentObservation;         // 観測本文。Debug では伏字化し、アクセサ経由でのみ読む
struct TaskAgentInferencePremise {   // port 入力。prompt は SecretScrubber の出力のみ
    delegation: DelegationId,
    task: TaskRef,                   // 依拠タスクリビジョン
    prompt: ScrubbedText,
    // dispatch 予定の論理本文全体を current mechanical condition と照合する。source ID は代用しない。
    data_use: Vec<RawId>,            // 採用目的＋全採用指示＋H-A.1 の既実行 facts の canonical source 相関。
                                     // 由来と参照 facts の identity の列であり、本文・hash を含まない。重複は許容し、
                                     // source ID で entry identity を dedupe しない。attempt claim が現在の
                                     // 消去条件と同一トランザクションで coverage を照合する。
}
enum TaskAgentInferenceOutcome {
    Produced {                       // 1 回の inference turn の provider 出力。final Task result ではなく、execution を seal しない
        output: TaskAgentOutput,
        adoption_consent_current: bool,
    },
    ResultUnavailable { ticket: InferenceTicketRef }, // 送信済みだが旧秘密の除去を証明できない。本文なし・外部作用再実行なし
    StaleTaskPremise,                // タスク前提の不一致（リビジョン前進・terminal・execution seal・行欠如を含む）。provider へ送信していない
    NotSent(TaskAgentNotSent),
    Aborted,                         // 呼び出し元のローカル協調停止で turn を終えた。claim 前の fast-path で
                                     // abort を観測した場合は claim を行わず、claim が durable に Started と
                                     // 確定した場合は利用実績の記録を完了してからこの回答を返す。durable な
                                     // cancel 受理とは別事実であり、provider I/O や外部作用が停止したことを
                                     // 意味しない（NotSent へ丸めない）。
}
struct TaskAgentOutput;              // 推論側で ticket の旧値を含めて scrub 済みの出力のみから作る。Debug は伏字化
enum TaskAgentNotSent { SetupIncomplete, NotInAllowlist, ConsentStale, OverLimit, EvaluationConsumed, DataUseHeld, CompanionUnavailable, DispatchWindowExpired, DispatchCapChanged, DispatchCostHeld, DispatchAborted }
// DataUseHeld（現在の消去条件による送信拒否）は、推論側 NotSentReason::DataUseHeld と対で
// Stage 4 erasure-currentness foundation（canonical current-condition store + AU14 claim 内の
// data-use 照合）が実装済みです。
enum TaskAgentInferenceError { InferenceUnavailable { reason: String } } // 技術的失敗（本文・秘密を含めない）
trait TaskAgentInference: Send + Sync {
    // 論理入力の上限（Unicode scalar values）。orchestrate は最新 exchange を残して古い exchange 全体を
    // 固定の omission note 付きで落とし、この予算へ収める。単一 exchange が予算に収まらない場合は黙って
    // 切り詰めず、port（dispatch）の絶対上限に委ねて NotSent(OverLimit) として送信前に拒否させる。
    fn input_budget(&self) -> usize;
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
    ResultUnavailable { ticket: InferenceTicketRef }, // provider 利用は記録済み。turn 本文・Action 要求・finalization なし
    TaskTerminal { task: TaskId, progress: TaskProgress }, // Task が terminal のため claim を拒否。provider へ送信していない
    ExecutionSealed { delegation: DelegationId }, // delegated execution が final result で seal 済みのため claim を拒否。
                                     // provider へ送信していない。Task lifecycle の terminal とは別（Task は InProgress のままであり得る）
    StaleTaskRevision { current: TaskRef }, // 依拠リビジョンが前進済み。provider へ送信していない
    MissingTask { task: TaskId },
    MissingDelegation { delegation: DelegationId },
    InstructionSourceMissing {       // 採用指示本文の canonical source が存在せず、本文を解決できない。
        entry: TaskContextEntryId,   // 採用 identity。context 順で最初に解決できなかった項目
        source: RawId,               // 解決できなかった由来レコードの参照
    },                               // 本文の捏造・指示の黙殺・TaskContextEntry の削除/書換え・provider I/O のいずれも行わない
    NotSent(TaskAgentNotSent),       // setup 不足・許可リスト外・同意失効・入力上限・利用済み評価・
                                      // data-use hold（現在の消去条件。DataUseHeld。Stage 4
                                      // erasure-currentness foundation が AU14 claim 内の data-use 照合で判定し、StaleTaskRevision 等へ丸めない）、
                                       // または dispatch 前の UTC window 失効 / cap 変更（旧 ticket の 0 byte と Released を確認済み）
    Aborted,                         // 呼び出し元のローカル協調停止で turn を終えた（出力なし）。claim 前の
                                     // fast-path で abort を観測した場合は claim を行わない。claim が durable に
                                     // Started と確定した場合は provider I/O は開始済みかもしれず、不確定利用
                                     // 実績（計測不明の usage fact）はこの回答の前に記録済み。Task progress や
                                     // 外部作用の停止を主張せず、TaskTerminal / ExecutionSealed /
                                     // StaleTaskRevision / NotSent へ丸めない（durable な cancel 受理とは別事実）。
}

// 採用指示本文の解決 port（作業側が定義する。Host 結合ルート apps/ene-core が
// 個体調整 owner の History / 第一者管理 activity の単一レコード bounded read へ写す）。
// 作業側は個体調整・会話履歴の具象型（HistoryMessage / HistoryRole / CompanionId）を import せず、
// 不透明 ID と作業側の写像語彙だけを受け取る。直接依存（ene-task → ene-companion）は CM §9.1 が禁止し、
// caller が本文列を組み立てて渡す形は canonical source の検証を迂回できるため採用しない。
struct TaskInstructionSourceRecord { // 本文の正本は kind に対応する History / activity のまま
    kind: TaskContextOriginKind,
    source: RawId,                   // origin.source と一致することを作業側が検証する
    companion: RawId,                // 発言主体（AssigneeRef.companion と同じ RawId 空間。ドメイン newtype へ変換しない）
    role: TaskInstructionRole,       // 会話履歴 role の作業側写像。作業側が Owner であることを検証する
    text: String,                    // 発言本文。Debug では伏字化し、正本を複製しない
}
enum TaskInstructionRole { Owner, Companion }
enum TaskInstructionSourceError {    // 技術的失敗（本文・秘密・provider 応答を含めない）
    SourceUnavailable { reason: String },
}
trait TaskInstructionSource: Send + Sync {
    // origin.kind で History / activity を選び、origin.source（PK）を直接引く bounded read。
    // timeline 全読込・load_recent_timeline・lookup_command から探す実装を禁止する。
    // 欠如は Ok(None)。malformed durable row は Err（フェイルクローズ）とし、合成値を返さない。
    async fn load_owner_instruction(&self, origin: TaskContextOrigin)
        -> Result<Option<TaskInstructionSourceRecord>, TaskInstructionSourceError>;
}

// 論理入力の組み立てと検証（順序固定。1 inference turn につき 1 回）:
// 1. load_task の context から AdoptedInstruction 項目を順序のまま取り出す。順序は load_task が返す
//    目的項目の後に続く (reference.revision, entry_id) 昇順であり、source ID での再ソート・dedupeをしない。
// 2. 各項目に origin.kind = OwnerConversation または OwnerManagement を要求する。本文解決 producer の存在しない
//    Spontaneous / ScheduleOccurrence の項目は、producer を持つスライスが同じ設計変更で解決を追加するまで
//    fail closed の技術的失敗として拒否し、黙ってスキップしない。
// 3. TaskInstructionSource で本文を解決し、kind/source 一致・role = Owner・companion = assignee を検証する。
//    参照先 record の不在（Ok(None)）は InstructionSourceMissing、対応関係の破損（source 不一致・role 不一致・
//    別 Companion の行）と読み出し失敗（TaskInstructionSourceError）は技術的失敗（fail closed。stale へ丸めない）。
//    読み出し失敗・対応関係の破損は既存の TaskAgentTurnError::InputUnavailable に写し（reason は固定クラス。
//    本文・History 行・秘密を含めない）、enum を増やさない。scrub 失敗も同じ InputUnavailable である。
// 4. 目的本文 → 採用指示本文を context 順に並べた 1 つの論理入力を組み立て、SecretScrubber を 1 回だけ通す。
//    目的本文と指示本文の境界は固定の framing で区切り、本文の順序・件数を変えず、空の指示集合では
//    目的本文のみを論理入力とする。prompt の具体的文言は実装の自由度だが、境界の表現は全 turn で同一にする。
//    実行スライス D の tool loop が replay する実行ローカルの Action transcript（provider 出力の要求と
//    Action owner の観測の対、古い順）も同じ 1 つの論理入力へ加わり、port の input_budget に収めるため
//    最新 exchange を残して古い exchange 全体を固定の omission note 付きで落とす（transcript は canonical
//    source ではないため、この削除で data_use 相関は変わらない）。単一 exchange が予算に収まらない場合は
//    transcript を黙って切り詰めず、port（dispatch）の絶対上限による NotSent(OverLimit) の送信前拒否に委ね、
//    silently shortened observation から答えさせない。
//    Stage 5 は H-A.1 の既実行 facts も全 turn に加える。これは上記 transcript の省略対象にしない。
// 5. ScrubbedText のみを TaskAgentInference へ渡し、data_use（目的・指示・既実行 facts の source 相関）を
//    attempt claim の前提として運ぶ。AU14 claim が依拠 TaskRef を atomic に比較するため、本文 read の後に
//    steering が勝った場合は古い prompt が provider へ届かない（CCT §7.2）。同じ claim は Stage 4
//    erasure-currentness foundation の canonical current-condition store と data_use の coverage も同一
//    トランザクションで照合し、消去条件が先に durable なら provider I/O を開始しない
//    （V-11 Case 11/12/14/15。本文を含む送信経路は foundation の実装後にのみ enable する）。
// 6. TaskContextEntry の作成・削除・書き換え、task_context_entry への本文複製、指示本文の cache は行わない。
//    消去条件による refusal を InstructionSourceMissing / InputUnavailable / StaleTaskRevision へ写さない。
//    本文を解決できない turn は provider I/O を開始せず、生成した論理入力や未解決の本文をログ・Debug・
//    技術的エラーへ出さない（HistoryMessage / TaskInstructionSourceRecord / TaskAgentInferencePremise の
//    Debug は本文を伏字化する）。
// `TaskAgentNotSent` / `TaskAgentInferenceError` は推論側 `NotSentReason` / `InferenceTechnicalError`
// と同じ意味の語彙を作業側に写したもの。`StaleTaskPremise` は作業側が委任・現在のタスク・
// delegation の final result（seal）を再読込し、委任が不在なら MissingDelegation、タスクが不在なら
// MissingTask、final result が durable なら ExecutionSealed、progress が terminal なら
// TaskTerminal { task, progress }、タスクリビジョンが前進していれば StaleTaskRevision { current } へ写す
// （判定の所有は作業担当に残る。terminal と seal を revision stale へ丸めない）。
// 採用指示本文の参照先 record 欠如（InstructionSourceMissing）は Ok 側のドメイン判定であり、本文を捏造・
// 黙殺せず、TaskContextEntry も書き換えず、provider I/O も開始しない。source 不一致・role 不一致・
// 別 Companion の行・本文解決 producer の無い origin kind・読み出し失敗は技術的失敗（fail closed）であり、
// stale や InstructionSourceMissing へ丸めず、既存の TaskAgentTurnError::InputUnavailable（reason は固定クラス）に写す。
// 参照先 record の不在そのものは TaskContextEntry の破損ではないため、TaskTerminal / StaleTaskRevision と同じ
// Ok 側のドメイン判定として返し、TaskContextEntry の削除・retire や本文 cache で「修復」しない。
// final result を提出する finalization 境界（TaskAgentResultArrival）は `Produced` とは別であり、
// orchestrate がそこで TaskResultId を発行し、AU15a で安全な本文または body-free の
// erasure-held 到着事実と seal を durable 化する。後者は採用判定へ進めない。
// どの provider 出力が final かを決める tool loop の低レベル protocol は本スライスでは固定しない
// （intermediate output を AU15a へ入れて execution を seal してはならないことだけを固定する）。
```

**AU14 / AU15a の消去時本文境界**: [Targeted Deletion lifecycle §7.1](targeted-deletion-lifecycle.md#71-論理本文と完了後の由来バリア) に従います。`data_use` は canonical source identity の durable 相関であり、tool transcript の内容を覆うものではありません。`TaskAgentActionExchange` の request と observation（Workspace read の本文も含む）を選択・省略した後、実際に provider に渡す単一の論理本文を current mechanical condition で照合します。scrub はその代わりになりません。AU14 claim は source coverage に加え、論理本文全体の mechanical coverage と由来 cut を同じ短い `Immediate` 比較点で判定し、covered / 判定不能なら `NotSent(DataUseHeld)`、provider 0 バイトとします。検索材料は保全・消去 owner に留め、Task / Inference の premise・attempt・`data_use`・retry・audit へ本文や hash を複製しません。provider adapter が後から prompt を追加・改変する場合も最終 dispatch 本文を同じ gate にかけます。claim 後の削除開始には dispatch 直前の再照合（各再送・継続も同様）を要し、旧 claim を無期限の送信許可にしません。claim が先に確定した事実は残し、送信が実際に始まったかは utilization の事実に従います。

`TaskAgentResultArrival` は credential owner が mint した `ScrubbedText` とそれに結合した `CredentialSetRevision` を渡し、raw final answer は到着処理のメモリ上だけで扱います。未確定の到着に限り、AU15a は到着 record と seal の **同じ** `Immediate` transaction で証明の revision と引数の整合・現在の credential-set revision・delegation の全推論試行が `verified` であることと旧 used-version authoritative set と証明 set の一致を先に照合し、古ければ `TaskResultArrivalOutcome::StaleCredentialSet { current }` として本文も seal も書きません。確定済み行への同一 ID retry は帰属を照合し、本文が残る `recorded` では到着本文も比較して不一致なら技術的エラー、同一なら既存 disposition を返し、本文のない `erasure_held` / `body_erased` / `legacy_body_unverified` では帰属だけを比較して現在の disposition を返し、外部作用を再実行しません。未確定なら bounded な再 scrub / 再提出を行い、上限到達は domain refusal とします。同じ transaction で active source / mechanical condition と完了後の body-free provenance cut も比較します。covered / 判定不能 / 旧由来を無関係と証明できない場合は Stage 6 の V44 `task_result` に `body = NULL, arrival_disposition = erasure_held` として identity・元 delegation / TaskRef・到着時刻のみを保存して seal し、`TaskResultArrivalOutcome::ErasureHeld { result }` を返します。covered 本文を結果正本・通知・ログ・監査・再試行 cache に保存しません。V44 の `body` は nullable で、`recorded` のみ非 NULL、`erasure_held` / `body_erased` / `legacy_body_unverified` は NULL（PR §4.5 の closed disposition と DB `CHECK`）。`erasure_held` は到着時保留・採用なしで吸収的です。事後 `body_erased` の retry は消去後の状態を返して本文を復元せず、別 ID の同一 delegation への二重 final は引き続き拒否します。AU15b は `erasure_held` について検証済み `task_result_attempt` 相関を記録しても `TaskResultAcceptance::ErasureHeld { result }` を返し、`adopted_revision` と Task progress を変更しません。`body_erased` / `legacy_body_unverified` の未採用結果も再採用せず、採用済み結果の再読込・再要求は過去の採用事実だけを返します。Action の `Unknown` と確定度は owner のままです。再表示・reconciliation は disposition を読み、本文なしの三状態を採用候補・本文復元候補へ含めません。cancel / stale revision による `RecordedToOriginalOnly` は本文を保持できた場合の帰属であり、消去による本文拒否を表しません。

削除開始の operation / cut 登録、AU14 claim と dispatch gate、AU15a 到着、保存済み結果への deletion participant の消去、finalizing / completion は同じ SQLite master 上で直列化します。削除開始が先なら送信・本文保存は保留、claim / 到着が先なら既存 attempt / 保存済み本文を deletion participant が掃除・検証します。保存済みの `recorded` が被覆された場合、削除参加は `body = NULL, arrival_disposition = body_erased` を同じ原子的更新で刻み、未採用・採用済みを問わず result ID・seal・依拠 TaskRef・`adopted_revision`・Action 相関・検証済み採用根拠と Task progress を維持します（対象本文を含む非依拠理由は別途 Task owner が消去し、body-free な根拠参照だけ残します）。`erasure_held` へ遡及変更せず、本文を後から復元しません。claim が先で arrival が deletion 後なら arrival は別途 hold します。完了後も cut に属する旧 provider / Task Agent / Action からの遅延本文を `active = 0` だけで保存せず、body-free な到着・利用・作用の事実を維持します。新しい Owner 提供の同一文字列は新たな provenance として受理可能です。検索材料は完了後に破棄し、完了の durable cut は対象本文 / hash を含めません。クラッシュ後に由来を検証できなければ fail closed の held にします。

Task の current result を API・未伝達・監査へ再報告する際も `TaskResultRecord` の disposition を確認し、`BodyErased` なら `body: None` のまま過去の採用 identity / revision・Action 相関と現在の本文消去を示します。`ErasureHeld` は到着時保留・未採用として区別します。本文を旧通知や監査のコピーから再構成せず、完了という過去事実から現在本文が存在すると推定しません。

V43 → V44 で旧 lease の秘密除去を証明できなかった既存結果は、PR §4.5 の `legacy_body_unverified` に本文なしで移します。既存 ID への再到着は帰属だけ照合して `TaskResultArrivalOutcome::LegacyBodyUnverified` とし、本文を再充填しません。AU15b の未採用行は `TaskResultAcceptance::LegacyBodyUnverified` で止め、採用済み行は過去の採用・完了だけを返します。V43 の非依拠理由も移行時に理由を消し `NonRelianceReasonDisposition::Erased` で読み戻します。新規 AU15a は移行専用状態を作らず、旧 V43 の本文や理由を通常の読戻し・報告・バックアップ経路から再導入しません。

Stage 6 の採用時入力 `VerifiedNonReliance { reason: String }` は理由を必須とし、Task owner が無作用失敗の代替達成と根拠を AU15b で検証・記録します。`TaskResultRecord.non_reliance` の読戻しは `RecordedNonReliance` であり、`NonRelianceReasonDisposition::Recorded` は `reason: Some`、`Erased` は `reason: None` のみ合法です。理由に対象本文が含まれるとき、Task owner の削除参加は理由本文を消去し disposition を Erased へ原子的に進めます。採用事実・attempt / evidence identity は body-free な参照として残し、参照先の本文も各 owner が消去します。読み戻しや再報告は消去済み理由を根拠参照、結果本文、通知、監査から再生成しません。

Agent を使わない Backup 専用 Task（Started occurrence または認証済み Owner Run now）は上記の delegation final result / AU15a / AU15b の producer に入れません。Task の保存済み由来と保全 owner の同じ point / output scope、Action owner の唯一の TaskBound backup 出力試行をそれぞれ元の owner に保持し、Task owner が同じ Task / point の検証済み成功 marker とその Action の `ConfirmedSuccess`、現在 revision・非 terminal progress・委任不存在・Task-wide `Unknown` が空であることを同じ短い commit で確認した場合だけ `Completed` にします。`adopted_result = None` はこの producer では正常であり、Agent final や `task_result_attempt` を仮造しません。通常 AU15b は保存済み Task 種別・由来を同じ transaction で照合し Backup 専用 Task を拒否します。`Unknown`・未完了・Cancel・復元 hold は完了を阻み、外部出力を自動再実行しません（PR V47 / AU10）。

#### H-A.1 中断 Task の明示 resume

resume は `ene-task` が所有する、残作業を進める新しい Owner 指示です。Host の再起動、認証成功、presence 復旧、報告の ACK はこの command の producer になりません。会話の個体調整または第一者管理操作が対象 Task と保存済み進捗を示し、Owner の明示指示を受けた場合だけ発行します。推論された「続けた方がよい」は指示の代わりになりません。会話の閉じた protocol には `[task-control] {"kind":"resume"}` だけを追加します。Task ID、revision、purpose、指示本文を model に出力させず、Host が会話で確かめた対象 Task とその現在の前提、当該 Owner message から構成します。対象が曖昧なら会話で尋ねて保留し、番号付き一覧の選択を要求しません。通常の `steer` を resume の別名にしません。

対象候補の取得は Task owner の read-only・上限付き query を通し、担当 Companion、依頼の対象手掛かり、会話との対応関係で絞ります。cancel は対象 Task と Owner の直接的な中断意図を確定し、曖昧なら受付を保留して確認します（既受理の cancel を解除したり同じ Task を再開したりしません）。Client の Task 一覧の先頭ページや LLM が作った Task ID を検索境界にしません。候補の `TaskRef`・目的・進捗は判断材料であり、steering / cancel / resume の受付では owner が現在の対象、revision と必要な前提を再照合します。曖昧な確認中の会話状態は Task の採用指示ではなく、再起動後は保存済み History と Task facts から再確認します。検索に失敗したら作用せず、その理由を会話へ返します。

```rust
struct ResumeTaskCommand {
    premise: SteeringPremiseRef, // expected TaskRef + その revision の purpose
    instruction: ResumeInstructionSource,
}
enum ResumeInstructionSource {
    OwnerHistory { message: RawId, currentness: OwnerMessageCurrentness },
    OwnerManagement { activity: RawId },
}
enum TaskResumeOutcome {
    Resumed { task: TaskRef, delegation: DelegationRef },
    ResumedBackup { task: TaskRef }, // 同じ Backup 専用 Task / source の agentless 残作業
    StalePremise { current: TaskRef },
    Superseded,                 // 会話の依拠 Owner 入力が既に新入力で置き換わった
    TaskTerminal { task: TaskId, progress: TaskProgress },
    AlreadyRunning { task: TaskId },
    HeldByUnknownEffects { task: TaskId }, // blocker は別の bounded report query で読む
    BackupOutputAlreadyAttempted { task: TaskId }, // 確認済み失敗は terminal 判定を優先し、同じ Task で再出力しない
    ResultAvailable { task: TaskId },     // 先に sealed result を再評価・表示する
    NeedsRevalidation(TaskResumeHold),
    MissingTask { task: TaskId },
    RevisionExhausted { task: TaskId },
}
enum TaskResumeHold {
    CompanionUnavailable, WorkspaceUnavailable, InstructionUnavailable,
    PermissionUnavailable, DataUseHeld, ExecutionUnavailable,
}
enum ResumeCommitShape {
    Normal { adopted_purpose_entry: TaskContextEntryId, adopted_instruction_entry: TaskContextEntryId, delegation: DelegationId, agent: TaskAgentEphemeralId },
    BackupAgentless, // Owner source を同じ TaskRef に結ぶ backup_task_resume 行のみ
}
// Task owner が通常 Task の新 revision entry ID と delegation/agent ID を発行する。
struct TaskResumeCommitPremise {
    command: ResumeTaskCommand,
    shape: ResumeCommitShape, // task_kind と source を transaction 内で再照合する
    accepted_at: WallClockWithTz,
}
// Task owner が上記 identity を一度だけ発行し、store の同期 commit_task_resume(premise) が
// Result<TaskResumeOutcome, TaskTechnicalError> を返す。通常 Task の r+1 / scope と
// Backup 専用 Task の同じ TaskRef / source は transaction 内で解決する。
// Host は CCT §7.4 の spawn_blocking 内で同期境界を確保してから commit を呼ぶ。
// 通常 Task だけ launch 予約を登録し、BackupAgentless に runner を渡さない。
```

`Resumed` は通常 Task の revision・新 delegation、`ResumedBackup` は Backup 専用 Task の同じ `TaskRef` に結ぶ新 Owner 再開指示の受付だけを意味し、provider 送信や外部作用の成功ではありません。commit 後の launcher 拒否や技術的障害は受理済みの事実と分けて表示します。拒否 outcome はすべて `Ok` 側で、Task の書き込み・launch ともにゼロです。入力として先に受け取った Owner 発言・管理 activity は、拒否されても起源記録として残ります。malformed durable row、他 Task の source/delegation を混ぜる不整合、参照の偽装は技術的エラーまたは ingress の境界拒否とし、空の前提へ丸めません。

**受理条件と順序**:

1. serving Host の認証済み入力境界で明示意図を確認し、同じ Task の保存済み progress、残作業、Action certainty を提示します。会話は Host が選択した Task に限り、候補が複数なら Owner の選択を求めます。管理操作は Task の明示選択を必須とします。再接続後に「最後の Task」や最新 delegation を推測して選びません。
2. 作業担当は目的・採用指示と通常 Task の現在 Workspace、Backup 専用 Task なら保存済み source・point / Action certainty を読み、再開に必要な source が解決でき、担当 Companion が Running で現行の実行境界を満たすか確認します。`OwnerHistory` は Owner role・同じ Companion・最新入力を照合します。`OwnerManagement` は個体調整が第一者操作から記録した activity を直接引き、`TaskContextOriginKind::OwnerManagement` として採用します。activity は明示選択した TaskRef・purpose、Owner の再開指示本文、受付時刻を持つ個体調整所有の入力記録です。同じ epoch の cmd_id は同じ activity を指し、retry で発行し直しません。作業担当は activity に保存された TaskRef/purpose と command の前提の一致も照合します。activity の本文はその 1 行だけに保持し、TaskInstructionSource は History と activity の各 PK に対する単一レコード読込を提供します。通常 Task のプロンプト内では Owner 指示として区別でき、source identity を AU14 の `data_use` へ渡します。
3. [CCT §7.4](concurrency-control.md#74-resume-は-task-種別ごとに残作業を受理する) の不分区間で、Task の存在・保存済み種別 / source、非 terminal、期待 revision・purpose、Owner 入力 currentness、同じ Task の launch 予約/実行登録がないこと、Task 全体の Action に `Unknown` がないことを比較します。source・担当個体・消去条件・通常 Task の Workspace または Backup 専用 Task の保存済み source / purpose・現在の setting / point・試行の読み取り前提（Schedule の変更・停止・削除は既存 Task の拒否理由にしない）を再照合します。取り消された同意で開始できるようにはせず、各利用の最終的な権限・利用量判定は従来の AU14/AU5 に残します。
4. 拒否の優先順は `MissingTask → TaskTerminal → StalePremise → Superseded → AlreadyRunning → HeldByUnknownEffects → ResultAvailable（通常 Task）→ BackupOutputAlreadyAttempted（Backup 専用 Task の確認済み失敗）→ NeedsRevalidation → RevisionExhausted（通常 Task）` とします。読み取り不能や不整合は成功へ推測せず技術的エラーにします。通常 Task の現 revision に採用可能な sealed result があれば、新しい仕事を作らず `ResultAvailable` を返し、既存の `reevaluate_result_adoption` へ渡します。`Unknown` がある場合は `HeldByUnknownEffects` を優先します。Backup 専用 Task に既存の `ConfirmedSuccess` 出力試行があれば同じ point の保全 owner の検証結果を再評価し、point 成功なら専用完了、回復不能な point 失敗なら Action の成功 fact を保持して失敗 producer の条件を確認します。Action が `ConfirmedFailure` なら同じ Task の再出力を拒否し、保全 owner が回復不能な失敗を検証済みなら `Failed` 確定を先に再評価します。`Failed` が先に確定すれば `TaskTerminal` を返し、未解決の影響や残作業の可能性が残れば `BackupOutputAlreadyAttempted` と既知の影響を報告して hold します。通常 Task の確認済み失敗によって result-local の採用条件を満たせない sealed result は resume を妨げません。再評価で完了が成立した後の resume は `TaskTerminal` です。
5. 通常 Task の受理は AU4 の revision 前進と AU3 の委任作成を **1 つの AU17** にまとめます。同じ Task を `r+1` に進め、目的 identity・既存指示・Workspace association は引き継ぎ、再開指示 entry と新 delegation/agent を記録します。Backup 専用 Task の AU17 は種別 / source / purpose と `TaskRef = (T, r)` を固定し、認証済み Owner 再開指示の body-free source identity・TaskRef・受付時刻を `backup_task_resume` に一回だけ記録します。再開本文は History / 管理 activity に残し、TaskContextEntry / delegation / agent を作らず、`Started → InProgress`（または `InProgress` 据え置き）にします。出力試行が Task 全 revision に 0 件で、point に回復不能な失敗 marker がなく出力準備済みなら、agentless producer が現在条件を再評価して未開始の作業を続けられます（point が既存なら同じ point を使い、外部 Action は新 AU5 のみ）。point が失敗確定なら無作用と残作業の条件を確認して `fail_task` を再評価し、新しい出力を予約しません。既存試行が `ConfirmedSuccess` なら同じ `TaskRef` / point の保全検証を再評価し、成功 marker が揃えば専用完了、回復不能な失敗なら Action 成功の事実を保持して `fail_task` の条件を照合し、新しい出力試行を mint しません。旧試行を付け替え・再送せず、新しい Action は Task 全 revision で出力試行 0 件かつ point が出力可能な場合に限ります。同じ source の retry は既存受付を返し、異なる source の新しい明示指示だけを別の受付 fact とします。
6. 通常 Task の commit に対応する今回限りの launch 登録だけを Host の既存 runner へ渡します。Backup 専用 Task は agentless producer に受理済みの同じ TaskRef を渡し、同じ point の残作業を現在条件で再評価します。DB の delegation 一覧から起動対象を復元しません。commit 後・起動前の crash も外部 work を自動補完せず、保存済み成功 facts だけの機械的な完了再評価を許します。続けるには同じ TaskRef に対する新しい明示 resume が必要です。

Stage 5 の **全 Task Agent execution・全 turn** で、現在の目的・全採用指示に加えて、過去の全 revision の操作（ActionAttempt identity・対象・操作種別・certainty）と記録済み result の帰属を Task/Action owner から読みます。先行 facts がなければ空とし、resume 専用 flag によって有無を判断しません。これを命令と区別した定型の「既実行 facts」ブロックにし、execution-local transcript の省略対象にしません。旧 tool transcript、provider session、古い解決済みパス・同意は復元しません。旧 result・attempt は新 delegation の `attempt_refs` に含めず、操作要求のキューにも転用しません。

読み込んだ ActionAttempt / TaskResult の canonical source identity は目的・指示の source とともに AU14 の `data_use` へ渡し、同じ claim で現在の消去条件の coverage を比較して durable に相関を残します。個体調整や推論側へ元記録の本文を複製しません。本文 read 後に消去条件が先に commit すれば `DataUseHeld` で 0 送信、claim が先なら既存の already-started use として削除参加の対象になります。未知の source 種別を送信可能とする既定値は設けません。

参照解決は bounded page と入力 byte 予算を使い、予算を超えた時点で読み出しを止めます。全 revision の必要な作用を網羅できなければ `InputUnavailable` / `OverLimit` で送信を保留し、事実を黙って落として続行しません。画面の report はページ分割できますが、推論入力の安全判断に必要な facts の欠落を画面表示で代替しません。

通常の resume には Unknown を無視する引数を設けません。客観的な証拠による既存の settlement 経路で確定するまで保留します。不明作用のリスクを認めた再実行は別の新規意図・新試行の契約であり、接続回復や resume の暗黙処理として実装しません。`Completed / Failed / Cancelled` は吸収的なままで、再実行は新しい Task です。

| 比較した案 | 整合性・実装量・重複状態・拡張性・安全性による判断 |
|---|---|
| 旧 delegation の loop を復元する | transient transcript の永続化と replay 判定が増え、開始済み作用を誤って再実行し得るため不採用 |
| 同じ revision に新 delegation と recovery generation を置く | 並列 AU3 には適するが、Task 全体の resume では旧結果の採用を止める別 gate が必要。TaskRevision と競合する状態が増えるため不採用 |
| 同じ Task の revision を進め、新 delegation を作る | 採用指示・revision CAS・既存 admission gate を再利用できる。旧作用の帰属と並列委任の一般契約も保てるため採用 |

表の新 delegation は通常 Task の選択です。Backup 専用 Task は revision を進めず同じ TaskRef に Owner 指示の受付 fact を結んで agentless に続けます。AU4 は保存済み `task_kind` / `backup_task_source` を変更せず、backup 以外の目的への steering と既存 point の TaskRef を切断する revision 前進を拒否します。新しい通常作業や出力をもう一度依頼する場合は Owner の新 Task 判断とし、既存 Task を種別変更して AU3 / AU17 の委任経路に入れません。

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
    HeldForErasure,                  // コミットが消去ゲートに拒否された。この由来は消去区間に属するため再試行せず、消去完了後の新しい由来だけを受け入れる
}
```

- クライアント識別子、会話ラウンドID、在席世代番号の細かな対応関係は、現在の開発ステージにおける経験形成では消費しないため、インターフェースには含めません（必要となるステージで再導入します）。
- 個人データ削除中による保留は `HeldForErasure` として形成処理が返します（削除ステージ A4/R2 の消去条件ゲート）。`MemoryChangeOutcome::HeldForErasure` と同じく、消去対応は操作完了後も残るため、同じ由来を再試行・再クレームせず、消去完了後に新しく提供された由来だけを新しい経験として扱います。保存禁止制約による `HeldByErasureOrConstraint` の制約部分は、制約を本格的に扱うステージで安全に再導入します。
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
    source: UndeliveredSource,       // CI §5.2 の canonical fact 参照（BackupTaskCompleted / BackupTaskResume を含む）。本文を持たない
    round: Option<RoundId>,
    presence_generation: Option<PresenceGeneration>, // 発生時の帰属がある場合だけ
}

enum ReportStatusTransition {
    PresentationStarted,            // 送信前に Pending → PresentationUnknown
    Presented,                      // 有効な実提示 ACK の後にだけ確定
    AlreadyPresented,               // 同じ事項への再 ACK。書込なし
    MarkedPresentationUnknown,
    NotPresented,                   // 現在の receipt で未提示と確認できた。Pending へ戻す
    StalePresentation,              // receipt / Round / connection / 世代の不一致
    StaleSource,                     // 元記録がない。Presented にはしない
    HeldByErasure,
}
// ネットワークの接続成功、表示データの送信完了、タスクの完了だけで「報告完了」にしてはなりません。
// 端末の画面上に実際に提示されたという観測事実に基づいて状態を更新します。
```

- マスターデータは個体調整担当が保持します。タスク由来の未伝達情報であれば、タスク記録への参照を保持します。ユーザーへの報告用要約は単なる派生データです。永続化は永続化グループB（`undelivered`）の不可分トランザクションで行います。

登録は PR §4.6 の各 producer の commit と不可分に行います。Backup 専用 AU17 の `BackupTaskResume { owner_source_kind, owner_source_id }` は受付行、agentless 完了の `BackupTaskCompleted(task_id)` は検証済み point / marker / Action による `Completed` CAS と同じ commit のみで登録します。宛先は Task の担当 Companion と照合し、Client 不在でも保存します。Task/Action の進捗本文を個体調整へ複製せず、異なるドメインの ID は composition root が意味を確認して受入側の raw reference へ写します。相手クレートの concrete repository を呼び合いません。元 source が示す report の本文は Task/Action/History の read port から組み立てます。

### H-H Schedule 設定・到来・各回対応

```rust
struct CreateScheduleCommand {
    owner_selection: OwnerSelectionRef,
    assignee: AssigneeRef,           // タスク側前提（CompanionId を直接インポートしない）
    content: ScheduleContentRef,     // 実行内容・初期ワークスペース入力
    timezone: ScheduleTimezoneRef,   // スケジュール作成時のタイムゾーン（勝手に暗黙変換しない）
}

enum OccurrenceStatus { Missed, Started, Unclassified }
enum ScheduleIntervalKind { MissedEligible, Uncertain }
enum ScheduleIntervalId { Host(RestoreGeneration, HostEpoch, IntervalSeq), CompanionStop(StopId, IntervalSeq), VersionCut(ScheduleVersion) }

struct ScheduleClassificationPage {
    schedule: ScheduleId,
    version: ScheduleVersion,
    interval: ScheduleIntervalId,
    kind: ScheduleIntervalKind,
    upper_utc: UtcInstant,
    after: Option<ScheduleOccurrenceKey>,
    limit: PageLimit,
}

struct ScheduleOccurrenceFact {
    schedule: ScheduleId,
    occurrence: ScheduleOccurrenceId,
    status: OccurrenceStatus,
    started_task: Option<TaskId>,    // 各回の実行は、独立した新しいタスクとして発行される
}
struct RunNowBackupCommand {
    schedule: ScheduleId,
    instruction: OwnerInstructionSourceRef,
}
// スケジュールの作成依頼そのものを、永続的な特別実行権限トークンとみなしてはなりません。
// 各回の実行タイミングにおいて、その時点の最新の権限制約を改めて再評価します。
```

Run now は `RunNowBackupCommand` の認証済み Owner source（会話なら Owner role・元 message の accepted sequence と最新 accepted Owner input、管理なら認証済みの明示操作 source とその現在性）を PK で確認し、指定 schedule の現行版・有効な設定・担当 Companion・canonical `Running`、削除 / restore / Host hold と現在のルールを AU2 と同じ短い比較点で再確認して独立の Backup 専用 Task 一式と source を不可分に保存します。過去の due 回・`Missed` / `Unclassified` を実行する要求ではなく、occurrence 行や分類 cursor を読み替え・更新しません。同じ source の重複受付は既存 Task を返し、source の別 schedule / Task への再利用は拒否します。確認待ちは Task を作りません。

TaskRepository の `TaskCreationPremise.kind = BackupExport { source }` は上記 Run now の `create_task` と AU18 の予定回で共有します。後者は occurrence の `Started` / Task 参照・区間 cursor と、Task の種別・初期 revision / context・`backup_task_source` を作業 owner の同じ短い writer commit に入れ、独立の Task 作成呼出しや後追い source 書込みを行いません。Run now も Task 種別・source・Owner 指示の帰属を一括比較してから公開します（PR AU2 / AU18）。

Schedule の版変更・停止・削除は将来の起動条件だけを変更します。AU18 / Run now の Task 作成が先に確定した場合、後の Schedule 操作は保存済み Task / source・point・試行を取り消さず、出力・明示 resume の gate に「現在も同じ Schedule 版・有効な設定」を追加しません。個体の Stop / Deletion、Task Cancel、現在の Permission・backup_setting・消去 / 復元保留はそれぞれ独立した gate です。削除済み schedule の再生成を求めず、歴史的な source の対応を照合します。

Run now の Owner source は Task 作成時に認証・対象 schedule・会話ならその時点の最新 accepted Owner input として検証し、その帰属を durable に保存します。以後の Permission 評価・AU5・最終作用 gate は元 source の同一性・非改ざん（管理操作 identity 自体の失効・偽造は拒否）と現在の Task revision / progress・権限を照合しますが、無関係な後続 Owner 会話入力を元 source の失効や再認証要求として扱いません。同じ Task の Cancel / steering が先なら既存の terminal / revision gate で拒否します。明示 resume の新 Owner 指示だけはその AU17 受付時に改めて currentness を照合します。

到来の受付は `(schedule_id, 発生キー)` を安定したキーとし、発生キーには時刻条件の `schedule_version`、予定 UTC 時刻、DST の重複時刻を区別する値を含めます。作業担当は `schedule` の現在の版・時刻条件・一時停止状態、担当個体の durable な `Running / Stopped / Deleted` と削除開始 hold、Host epoch / 停止・不確実区間、restore hold、旧版の退役区間を **同じ SQLite master の短い比較・commit 点**で照合します。現行版の `due`（予定 UTC 時刻が比較点の現在時刻以下）で、今回の連続稼働区間に属する未記録回は、通知が予定時刻を過ぎても DB 境界で bounded な keyset ページから一意に admission し、`Started` と新 Task 一式（AU2）・`started_task_id` を不可分に確定します。未来回は先読みで開始しません。事前の `Running` read や scheduler のキャッシュは実行許可になりません。個体 Stop または durable に証明された Host 停止の区間内の未記録回だけを `Missed`・Task なしとします。旧版は新 Task を起動しませんが、版切替前の回も停止の証拠なしに `Missed` と推測しません。停止開始が欠ける crash 前後の未記録回は `Unclassified`・Task なしで保護し、Owner へ未分類と提示します。時計逆行・snapshot / cursor 欠落では確定も admission も保留します。Stop が先ならその停止区間の回は `Missed`、開始が先なら既存 `Started` を維持します。削除開始 hold / `Deleted` が先なら新 Task を作らず、古い通知から設定を復活させません。再到来は既存結果を返すだけです。

個体 Stop / Deletion、Host shutdown / stop と startup の境界は到来と同じ writer の比較点を通ります。Host は serving 開始時に `host_epoch` と固定上端を durable 化し、明示 shutdown / stop（sleep の事前通知を含む）では先に admission gate を閉じて停止下端を durable 化します。crash で停止下端がない前 epoch は、分類済み安全境界から停止したとは見なさず、未記録回が属し得る稼働開始から startup 固定上端まで `Uncertain` とします。作業担当は Host の区間を各版の有効期間と交差させ、`(schedule_id, schedule_version, interval_id)` に区間種別・上端・snapshot・cursor を保存します。9 時から 10 時への変更も旧版の交差を残します。区間ごとに `(予定 UTC 時刻, DST 識別子)` の keyset で最大 N 回を上流から生成し、証明済み停止は `Missed`、不確実は `Unclassified` として cursor と同じ短い commit で確定します。既存 `Started` を上書きせず、他区間の cursor を流用しません。Resume は停止中の回の分類後に最後の比較を行って `Running` を公開し、startup は競合回の整理まで admission を保留します。`Unclassified` は新しい客観的な停止証拠がある場合のみ writer で再評価し、根拠なく `Missed / Started` にしません。Owner に区別して通知・表示し、Run now は独立した新 Task として現在条件を照合します。Run now は旧発生キー・状態・cursor を消費しません。削除 hold や post-restore hold は別の gate として維持し、復旧や一括有効化だけで解除しません。全 schedule の事前 materialize、長い DB lock、外部 I/O は行いません（PR §4 Group D・§6・§7 AU18、CCT §5）。

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
    write_purpose: Option<WorkspaceWritePurpose>, // Create/Edit の用途。評価 ID に束縛する
    save_target: Option<OwnerConfirmedSaveTargetPremise>, // FinalDeliverable の場合のみ現在値を依拠
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
- **binding と単一使用**: 評価 ID は候補 fingerprint（delegation・task・task_revision・workspace・operation・write_purpose・resolved_target と、最終成果物なら依拠する Owner 確認済み保存先 ref / 版・folder）に束縛し、`consume` は一致する fingerprint での一度だけ成功します。同じ ID を別の対象・用途・操作に使い回すことはできません。
- `resolved_target` の文字列は判断・監査用の記述であり、実行用 capability ではありません。実行・拡張担当が検証済み実対象を保持したまま候補を構成し、評価 ID の consume と AU5 の開始後も同じ対象を使います。候補の文字列と一致する別の path を再解決して使うことや、保持していた対象を別の実体へ差し替えることはできません。
- **AU5 start への接続**: 実行・拡張担当は対象を解決した後に `authorize_action_use` を呼び、`AllowForThisUse` の場合だけ評価 ID を consume してその opaque な `RawId` に落とし、`AttemptCommitPremise.relied_evaluation` として AU5 の挿入トランザクションへ渡します。挿入トランザクションは同じ不分区間で委任・依拠タスクリビジョン・現在のタスク・現在の workspace_assoc・委任スコープを再照合するため、評価の前提が動いた場合は開始されません。永続化された `relied_evaluation` と試行行の対応関係（操作・実対象・委任・依拠 TaskRef）が、その判断の durable な binding です。同じ評価 ID での二度目の開始は技術的エラーとして拒否し、二重開始を許しません。
- **Create / Edit の用途と保存先**: `write_purpose` は `Create / Edit` に必須、`List / Read` では `None` です。`save_target` は `FinalDeliverable` にだけ必須で、他の用途では `None` です。Task Agent の出力文字列やファイル名ではなく、採用目的・Owner 指示と操作の実用途に照らして Action owner が確定し、候補・評価・AU5 前提・最終作用まで一致させます。`IntermediateWork` は確認済み Workspace 内の作業用ファイルに限り、外部から読んだ原本の上書きや最終成果物の保存に転用できません。`FinalDeliverable` は Workspace 範囲とは独立に確認済みの現在保存先 ref が必要です。作成時の `save_target` が `None` で後続の Owner 確認済み現在 ref も無い場合、権限 allowlist に Create/Edit があっても `AskOwner` とし、AU5 でも試行なしで拒否します。現在 ref / 版と保持した実対象の一致は評価・AU5 の同じ不分区間で照合し、作用直前にも再照合します。未確認・失効・対象不一致なら最終成果物は作用なしで確認待ちとし、評価 ID や古い Workspace snapshot を保存先の権限にしません。
- **判断記録との区別**: `permission_evaluation` の評価ログ行（D2）、ルール・Deny・毎回確認の永続状態、オーナー確認（AskOwner）は、それらの producer を持つスライスが同じ判断点に追加します。このスライスは代役の許可・placeholder 列を置きません。

後続 Direct slice では `ActionUseCandidate` / `CurrentActionPremise` の Task 四列を `ActionAssociation::TaskBound` と `Direct` の閉じた対応に前進させ、Direct の Owner source kind / ID・`Option<CompanionId>`・許可対象 scope・`Option<SystemActionOperation>` と操作 / 保持した実対象に評価 ID を束縛します。`companion = None` は認証済み第一者管理 source と `ManualBackupExport`（手動バックアップの外部出力）、`CharacterPackageImport`（個体生成前も含むパッケージの外部ファイル Read）、`CharacterExport`（Companion 非依存の静的定義の外部公開）のいずれかが一致する場合のみ受け付けます。用途・source・許可操作（順に外部 Create、Read、外部 Create）と保持した実対象・対象 scope を Permission owner が今回限り評価し、AU5 と作用直前にも比較します。対象外の操作、会話 source の `None`、用途欠落を拒否し、単に管理操作が届いたことを包括許可にしません。`Some(companion)` の Direct は system operation を持たず、会話起点・通常の管理起点とも従来の Running gate を通します。TaskBound 用の Stage 4 allowlist を Direct の既定許可として流用せず、producer のない操作は `Deny` / `AskOwner` とします。manual Computer Use も同じ single-use binding に従います。

手動管理バックアップの明示アームは保全 owner の `(Management(ActivityId), BackupPointId)` の一意な対応とその point の確定出力範囲・内容、Owner 指定の保存先、保持した実対象を候補 fingerprint / 現在前提に含めます。同じ管理 source の別 point や別評価での再出力は認可しません。Permission の single-use 評価 ID が新しくても source 当たりの試行一件制約を置き換えず、既存 `Unknown` は Owner に二重出力リスクを示して保留します。別の明示要求なら新 source / point として現在の許可・リスクを評価します。

V47 の `TaskBound { scope: BackupTaskExport { .. } }` は Permission owner の独立した明示アームで、Started occurrence または認証済み Owner Run now から作成された Backup 専用 TaskRef と保存済み source、保全 owner が確定した backup point / output scope、Owner 選択の現在 `backup_setting` 保存先と revision、外部 Create と保持実対象にだけ single-use 評価 ID を束縛します。Task 四列の Workspace allowlist は流用しません。Task の保存済み source / purpose（Run now の認証済み Owner role / 元操作 identity または Started occurrence の帰属）・point / scope / setting の現在性と担当 Companion の Running、現在の権限・ルール・確認待ち・削除 / 復元保留を評価・AU5・作用直前の全てで照合し、point の出力内容 / scope が準備済みで回復不能な失敗 marker がないことも同じ master の比較点と作用直前に確認します。point の失敗確定が先なら Task の `Failed` commit 前でも試行・作用を始めません。保存先の変更・未選択・権限失効では試行・作用を止めて再評価または `AskOwner` とし、設定そのものを恒久的な許可にしません。出力試行が 0 件で外部未開始と証明できる point は保全 owner が同じ point / TaskRef / source / 論理的な対象範囲を保ったまま現在の保存先・保護設定と revision を CAS で再確定し、以前の評価を捨てて必要なら未公開の一時ファイルを作り直し、出力内容・実対象・権限を再評価できます（PR §4.5）。既存試行または不明な作用があれば point / 保存先を付け替えず、同じ Task で再出力しません。Backup 専用 Task は AU3 を拒否し、通常 Task の Workspace 用途はこのアームに入れません。

通常 Task の委任に有効な Workspace 帰属がなければ Agent の作用も開始せず保留します。Backup 専用 Task は試行前も AU3 を拒否し、P1 が `Unknown` / failure の後の P2 を同じ Task から出力せず、別 point での再実行は Owner の明示判断に基づく新 Task でのみ行います。

V47 の候補と現在前提は `association: TaskBound { task: TaskRef, scope: Workspace { delegation, workspace } | BackupTaskExport { source: StartedOccurrence { schedule, occurrence } | RunNow { schedule, instruction }, backup_point, output_scope, setting, setting_revision } } | Direct { .. }` の同じ閉じた型を使います。`BackupTaskExport` の `write_purpose` / `save_target` は Workspace 成果物用の値でなく `None` とし、候補 fingerprint は上記の相関・外部 `Create`・保全 owner の確定出力内容の同一性・保持実対象と Owner 選択の現在保存先を含みます。current premise を AU5 と作用直前に同じ条件で再読込し、`backup_setting` の過去の値や schedule 設定を許可証にしません。

Stage 4 の K-B.1 は明示 allowlist と single-use 評価 ID のみを発行し、未導入の rule revision を仮の値で補いません。rule / Deny / 毎回確認の producer が導入されたら、評価の依拠 rule identity / revision と現在の意味を同じ判断・AU5・最終作用 gate に接続します。AU5 で評価を consume した後の gate は同じ評価を二度 consume せず、保存した binding と依拠した判断が今も有効かを照合します。

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

#### K-C.1 ScrubbedText の構成所有者と合成境界

`ScrubbedText` の構成は `ene-credential` の scrub module だけが所有します。具象 `CredentialScrubber` は Host ではなく同 module に置き、Host composition root は既存の `CredentialRefRepository` / `CredentialSetRepository` と revision に束縛した immutable snapshot を供給する `CredentialStore` を注入します。別 registry は作りません。公開 guard 内で同じ snapshot の refs / bearer と revision を一緒に取得し、全登録値を longest-first で除去します。provider 出力は用途を問わず、元 ticket の `inference_attempt` が used-version 集合を `verified` として記録していることを確認します。Dialogue・Learning・Task Agent の途中結果は ticket の旧 `SecretVersionId` 集合を、Task Agent final answer は delegation の全 inference claims の verified な旧集合を body-free な相関から読み、credential owner 内で旧 item を秘密除去専用に解決して現行値と一緒に除去します。出力 proof は現在 revision と ticket（final では delegation）に束縛した verified 旧 version 集合を封止し、V43 の `unknown` を空集合として mint しません。必要な旧 item の取得不可は明示的な unavailable outcome とし、本文は返しません。本文の処理後に新しい revision や旧 version set を取り直して proof を昇格させません。snapshot を構成できない場合や空 bearer は本文を返さず fail closed とします。

保存済み provider 由来本文の再読込表示も現在の revision と全由来 ticket / delegation の proof を再照合します。本文 owner は閲覧可能な本文の由来と閲覧可能性を保存・retention・消去とともに確定し、credential owner の通常退役 cleanup は旧 version → ticket / delegation → 閲覧可能本文の索引付き bounded page を同じ master の cleanup fence で照合します。未完了 result / stream がなくても表示可能本文に必要な旧 OS item は残し、閲覧可能性がなくなるまで lease 解放や seal を削除根拠にしません。旧 item 不達・`unknown`・由来欠落では本文の再表示を unavailable とし、保存済み相関と利用実績を維持します。対象登録の Targeted Deletion は本文を body-free unavailable / erased として旧 item の必要性を閉じます（PR §4 Group K、Credential publication §3–5）。

`ScrubbedText` の本文・revision・provider 出力の ticket / delegation 旧 version set 証明は private とし、読み取り accessor と本文の消費だけを公開します。public literal、任意本文と revision / version set を受け取る constructor、Deserialize、可変本文参照、test feature による constructor は提供しません。`SecretScrubber` port の外部実装は失敗または credential-owned scrubber への委譲が可能ですが、自ら proof を mint できません。テストも repository / bearer の fixture をこの同じ具象境界へ注入します。この型は注入された credential authority に対する scrub の証明であり、任意の repository 実装が正規 Host authority であることまで型で証明するものではありません。production の authority 選択は Host composition root に残します。

会話・Learning は選択予算などのために先行 scrub した断片を使ってよいものの、合成後の本文全体を再び scrub 境界へ通してから推論へ渡します。断片の連結・見出し等で値が再形成され得るため、断片の proof と任意の合成本文だけから proof を作ってはなりません。先行断片がある場合、最終 proof の premise は全先行断片と最終 scrub の revision の最小値まで保守化します。この操作は既存 proof の revision を下げるだけで、本文の差し替えや revision の昇格を許しません。先行断片がない場合も最終本文を scrub します。Task Agent は既存の契約どおり、論理入力全体の単一 scrub のままです。

scrub proof は送信・保存 authority ではありません。既存の durable commit / inference attempt claim における credential-set revision の等値比較、approval / startup sweep と revision bump の不可分性は変更しません。合成中に revision が動いても、最終 scrub だけの新しい revision で古い断片の premise を洗い替えません。封止の compile-fail、同じ具象境界の redaction / failure、合成と revision drift を C1 で検証し、全域 no-secret / restart E2E の拡張は C2 / C3 に分離します。

#### K-C.2 稼働中の credential 更新

OS 保護ストアへの候補保存と有効化を分離します。有効 version の参照切替、既存本文の approval sweep、credential-set revision、操作 outcome は同じ master transaction で確定し、credential owner の公開 guard 内で対応する immutable snapshot を公開します。scoped use は claim と同じ version / revision に束縛し、claim 後に最新キーへ取り直しません。snapshot と実行中 lease の寿命、失効、partial failure、restart は [Credential publication](credential-publication.md) を正本とします。OS `put` 成功や登録のみで利用同意を成立させません。

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
    data_use: Vec<RawId>,            // 目的・指示の origin.source と H-A.1 の既実行 facts の source。
                                     // 本文・hash を含まない。claim が現在の消去条件と
                                     // 同一の不分区間で coverage を照合し、attempt 行へ durable に残す。
                                     // 削除参加はこの相関で already-started use を列挙し、遅延結果の再保存を防ぐ。
}

// 1 回の in-flight dispatch へのローカル・best-effort の協調停止トークン。canonical state ではなく、
// 再起動や別プロセスへ持ち越されず、その有無や signal の送信を中断受理・停止完了の権威にしません。
// dispatch は claim を試みる前の fast-path で abort を観測した場合は claim を行わず attempt も利用実績も
// 残さず Aborted を返します。claim が durable に Started と確定した後の abort は最終送信 gate で
// 未送信と旧 ticket の送信不能を証明できた場合だけ NotSent と予約 Released を同じ commit で記録します。
// それ以外は不確定利用実績を記録してから Aborted を返します（provider 待機中なら provider future を
// ベストエフォートで drop。記録不能は storage の技術的エラーで、clean な停止へ丸めません）。claim 自体が
// 拒否された場合（非 Started）は通常の pre-send refusal であり、Aborted へ写しません。呼び出し元が
// dispatch future を drop することは、この記録を skip させる停止の代用になりません。
struct DispatchAbort;

enum InferenceDispatchOutcome {
    Completed {
        arrival: InferenceResultArrival,
        adopted: bool, // 待機（await）完了後の結果採用の同意が正常に成立したかどうか
    },
    ResultUnavailable { ticket: InferenceTicketRef }, // provider 消費は記録するが旧秘密を除去できず本文は返さない
    NotSent(NotSentReason), // provider 0 byte の証明がある拒絶。claim 後なら試行と Released の記録は残す
    Aborted, // claim 前の fast-path で abort を観測した（attempt を claim しない）か、claim が durable に
             // Started と確定し、未送信を証明できない attempt をローカルの協調停止で終えた状態。
             // 後者は不確定利用実績（計測不明の usage fact）を記録してから返り、記録できない場合は clean な停止へ丸めず
             // storage の技術的エラーとして fail closed する。claim が拒否された場合（非 Started）は
             // 通常の pre-send refusal であり、attempt も利用実績も残さない。provider 要求・外部作用が
             // 停止したことは意味しない。未送信を証明できた claim 後の abort は NotSent として別に扱う。
}

struct InferenceResultArrival {
    ticket: InferenceTicketRef,      // 元の要求チケットおよび範囲との対応関係
    output_text: ScrubbedText,       // credential owner が現行値とこの ticket の旧 used-version set で除去した本文
    usage: UsageFactRef,             // プロバイダから報告された利用量（または計測不明の区別付き）
}
```

- provider が返した raw 本文は推論側の到着処理のメモリ内だけに留めます。`InferenceResultArrival.output_text` は credential owner が現在の全値と元 ticket の durable used-version 集合の旧値を除去し、現在 revision・ticket・検証済み集合を封止した `ScrubbedText` に限ります。Dialogue・Learning・Task Agent の途中 turn は同じ ticket の集合、Task Agent final answer はその delegation の全 ticket の集合を使います。V43 から移した `unknown` 試行は子行ゼロでも「秘密利用なし」ではなく、旧 item の取得不可と同様に本文を返さない `ResultUnavailable` とします。stream は各 chunk を独立な文字列として scrub しません。旧値と現行値のいずれも chunk 境界で分割され得るため、credential owner が完全一致を判定できるだけの未送信 suffix を保持し、前後の chunk をまたぐ秘密を除去した後の確定済み prefix だけを送ります。必要な最大保留長や bounded な照合を保証できなければ逐次本文を出さず、最終結果の安全な scrub まで保留します。queue への投入は送信許可ではありません。Client 送信 owner は各 chunk の実送信開始点で、Credential 更新 commit と同じ publication guard の下、現在 revision・使用版集合・送信前の消去条件・配送可能性の durable 記録を再照合します。application queue・codec / transport の待機中に proof が古くなった未開始 chunk は 0 byte に留め、元の論理本文を bounded に再 scrub するか本文なしで保留します。開始済み chunk は元の revision の送信事実として保持し、後続 chunk には再び gate を課します。未確認の raw chunk を Client へ先送りしません。保存 owner も現在 revision・使用版集合・現在の消去条件を確定点で照合します。外部作用・provider 利用量の事実は保持し、自動再送しません。

- `NotSentReason` と試行確定の結果は、Task Agent のタスク前提不一致（`TaskPremiseStale`）を同意・認証情報の失効（`ConsentStale` / `Stale`）とは別の列挙子として持ち、技術的エラーへ吸収しません。現在の消去条件による送信拒否（`DataUseHeld`）もまた別の列挙子であり、`TaskPremiseStale` / `ConsentStale` / `Stale` / `ExecutionSealed` / `TaskTerminal` へ丸めず、storage エラーとして扱いません（列挙子は Stage 4 erasure-currentness foundation が claim 内の data-use 照合と対で追加します）。作業側は `TaskPremiseStale` を受けて委任・現在のタスク・delegation の final result（seal）を再読込し、`MissingDelegation` / `MissingTask` / `TaskTerminal { task, progress }` / `ExecutionSealed { delegation }` / `StaleTaskRevision { current }` へ写します（判定の所有は作業担当に残ります。terminal と seal を revision stale へ丸めず、`TaskPremiseStale` に Task lifecycle の値型を import させません）。`DataUseHeld` は再読込による写像の対象ではなく、そのまま作業側の `TaskAgentTurnOutcome::NotSent(TaskAgentNotSent::DataUseHeld)` として turn を終えます。

K-G のprovider送信では、全置換された復元済みcap設定と、切替前に確保した復元前cap由来の短命なrestore safety contextを比較し、現在適用される実課金先・scope × UTC windowに限り現在費用と`cost_restore_gap`を照合します。復元前capを新マスターのcap設定としてマージせず、解除済みcapは再有効化しません。復元前cap由来の境界はOwnerの明示解除判断（一括有効化とは別）または対象window終了まで暫定評価・保留し、状態確保不能なら適用され得るscope/windowだけを安全側に保留します。capのないwindowはgapのみでは送信を止めず、日額のみなら月額unknownによって翌日の非重複dayを止めません。`NotSentReason` は予約後の UTC window 失効、現在の cap revision / amount・残枠による不適格、適用 cap window の費用現在性が不明なことによる dispatch 拒否をそれぞれ区別します。revision が変わっても現在の全適用 cap で送信可能ならそれを評価結果とし、旧判定を使い回しません。claim 前の cap 拒否は attempt / reservation を作らず、claim 後の拒否は provider 0 byte を transport が証明し元 ticket の送信を fence できる場合だけ、試行を未送信として終え、同じ短い commit で予約を `Released` にします。`NotSent` はこの場合の試行・解放の durable fact を消す意味ではありません。証明できなければ `Aborted` や `NotSent` に丸めず Unknown の利用として保持し、旧 ticket を自動再送しません。

Task Agent の推論 port は推論側 `NotSentReason::DispatchWindowExpired` / `DispatchCapChanged` / `DispatchCostHeld` を対応する `TaskAgentNotSent` へ写し、`TaskAgentTurnOutcome::NotSent` として返します。window 失効を `OverLimit`（トークン/費用上限到達）や `DataUseHeld`（消去中の送信保留）へ読み替えず、cap 不適格と費用利用保留も window 失効や storage error へ丸めません。`DispatchAbort` 後に 0 byte と旧 ticket の送信不能を証明したときは推論側 `NotSentReason::DispatchAborted` を作業側 `TaskAgentNotSent::DispatchAborted` に写します。Task Cancel が先なら `TaskPremiseStale` から `TaskTerminal` を導き、ローカル abort の有無を送信済みの証拠にしません。元の試行は未送信の事実として残し、続行が必要なら作業担当が現在の指示・Task 前提で新しい推論試行を明示的に開始します。

- **送信の手順**:
  AU14 の durable claim は provider first-byte の権限を固定しません。Task Agent の最終 dispatch gate は、Task の非 terminal progress、依拠する current revision、delegation の未 seal、現在の assignment / Capability 同意（撤回を含む）、認証情報セットと使用 lease の有効性、担当 Companion の Running / deletion hold、論理本文と `data_use` の現在の消去条件を、費用現在性・UTC window・全適用 cap の既存 gate と併せて first-byte に短く直列化します。Cancel・steering・seal・同意 / assignment 撤回・Stop・消去開始・cap 更新が先ならその旧 ticket からは送信しません。送信開始を区間内に確定できなければ解放して再照合します。0 byte と旧 ticket の送信不能を証明したときだけ `NotSent` と予約 `Released` を確定し、証明できなければ元 claim / `data_use` と予約を残して usage `Unknown` とします。既に送った byte は停止要求で取り消せず、以後の送信は各 dispatch gate を通します。provider 応答待ちに DB transaction / Task-wide lock を保持しません。
  Task Agent の担当 Companion が Stopped / Deleted / deletion hold なら、AU14 claim の `NotSentReason::CompanionUnavailable` を作業側 `TaskAgentNotSent::CompanionUnavailable` に写して provider へ 0 byte とします。これは Task premise stale・`Aborted`・`DataUseHeld` と異なる domain refusal です。claim 後に Stop が確定して provider first-byte より先に最終 gate が拒否した場合は、既存 claim と data-use 相関を残し、transport が 0 byte と旧 ticket の送信不能を証明できる場合だけ未送信と利用量予約の解放を記録します。証明できない消費を 0 とせず不確定利用として記録します。first-byte が先なら停止は best-effort で、送信事実を消しません。
  1. 受付ゲート（admission）が最新の同意・認証前提と、K-B の単一利用認可を確認して `Admission` を返します。Task Agent の admission は `(TaskAgent, Dialogue, TaskAgentTurn)` を固定した専用経路（`admit_task_agent`）のみで作り、`admit_dialogue` / `admit_learning` の代用を許しません。
  2. 試行の確定（attempt claim）が、保存された同意情報および認証情報セットとの一致を単一のトランザクションで確定した上で、トランスポート層を介して送信します。Task Agent の利用では、同じトランザクションで次の 5 条件を照合します: (1) 前提の `delegation` が `delegation` 行に存在する、(2) その行の `(task_id, task_revision)` が依拠 `TaskRef` 前提と一致する、(3) 現在の `task` 行のリビジョンが依拠リビジョンと一致する、(4) 現在の `task` 行の progress が非 terminal（`cancelled` を含む terminal 集合の補集合）である、(5) その delegation が seal 済みでない（その delegation の `task_result` 行が存在しない）。(1)(3)(4)(5) の不一致と行の欠如は `TaskPremiseStale` として送信前に拒絶し(2) の不一致や部分的・不整合な行は技術的エラー（fail closed）とします。さらに、`data_use` の各 source 相関と送信する論理本文全体が現在の durable な消去条件に覆われていないことを同じ不分区間で照合します（消去条件は enforcement より先に durable 化され、claim と同一 SQLite master 上の短い `Immediate` トランザクションで直列化されます）。coverage がある場合は `DataUseHeld`（data-use hold）として送信前に拒絶し、`TaskPremiseStale` / `ConsentStale` と区別して provider へ 0 バイトも送りません（この照合は Stage 4 erasure-currentness foundation が canonical current-condition store の照会として実装し、Stage 6 の Targeted Deletion producer は同じ store を拡張します）。claim が先に確定した attempt は `data_use` 相関つきの already-started use として durable に残り、その後の削除開始に対しては削除参加の停止・縮小・遅延結果再保存防止の対象になります。terminal を含む refusal を受けた作業側は、再読込で terminal を検出すると `TaskTerminal { task, progress }`、delegation の final result（seal）を検出すると `ExecutionSealed { delegation }` へ写します。同意・認証情報の不一致は従来どおり `Stale` で、`TaskPremiseStale` と区別します。
  3. 入力トークン上限は確定前に、プロンプト内の認証情報セット前提は試行確定と同一のトランザクションで照合します。
  4. 試行確定後のプロバイダへの非同期I/Oは長い transaction / lock を保持しません。ただし予約は後日の送信許可ではなく、各 provider dispatch（再送・継続を含む）の直前 gate で、予約の日額・月額 UTC window に加え、**実課金先に現在適用される provider / system cap の種別・scope × UTC window の現在費用と復元欠損期間、全適用 cap の現在 revision・amount と予約を含む current totals / 利用可能残枠、該当する restore cost hold**を同じ master の現在状態から再照合します（[Usage / Cost / Cap §9](usage-cost-cap.md#9-provider-send-admission-の-linearization)）。cap がない window の gap はそれだけでは拒否理由にしません。cap の引下げと first byte の間は短い送信開始境界で直列化し、cap update が先なら新 cap で拒否して provider 0 byte、first byte が先なら開始済み利用として帰属させます。transport がこの順序・0 byte を保証できないときは未送信と偽らず fail closed とします。gate を通過できず未送信を証明し旧 ticket の送信を fence できた場合だけ元の予約を release して試行を未送信として閉じます。開始済みの呼び出しは停止をベストエフォートで試み、確定しない消費は `Unknown` として予約上限を保持し、自動再送しません。次の試行には全適用 cap と同意・認証・Task・消去条件を新しい claim で再評価します。
  5. ネットワーク待機（await）後に同意状態が変化して結果を採用できなくなった場合は、生成結果の採用のみを安全に破棄し、利用実績の記録は確定した試行情報に従って正しく残します。
  6. 呼び出し元にローカルの協調停止トークン（`DispatchAbort`）がある場合の停止は、claim と利用実績の記録を所有する推論境界が行います。dispatch は claim を試みる前の fast-path で abort を観測した場合は claim を行わず attempt も利用実績も残さず `Aborted` を返し、claim が durable に `Started` と確定した後に abort が発火した場合は、最終送信 gate で 0 byte と旧 ticket の送信不能を証明できたときだけ元の attempt / `data_use` を残し `NotSent` と予約 `Released` を同じ commit で確定します。Cancel が先なら `TaskPremiseStale` を Task owner が terminal へ写し、ローカル abort 単独なら `DispatchAborted` とします。証明できない場合は不確定利用実績（計測不明の usage fact）を記録してから `Aborted` を返します（provider 待機中なら provider future をベストエフォートで drop。記録不能は storage の技術的エラーで、clean な未送信へ丸めません）。claim 自体が拒否された場合（非 `Started`）は通常の pre-send refusal であり、`Aborted` へ写しません。トークンは canonical state ではなく、再起動・別プロセスへ持ち越されず、`Aborted` は provider 要求・外部作用が停止したことを意味しません。呼び出し元が dispatch future を drop してこの記録を skip してはならず、`Aborted` を未送信の証拠なしに `NotSent` へ写してはいけません。Task Agent の turn では、この outcome が `TaskAgentInferenceOutcome::Aborted` → `TaskAgentTurnOutcome::Aborted` として写り、実行ループは provider I/O の停止を主張せずに turn を終えます（Task owner の durable な cancel 受理とは別の事実です）。
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
    attribution: UsageAttributionRef, // 実課金先・用途から全適用capと一時安全条件をownerが解決する
    upper_bound: UsageUpperBoundRef, // 想定される利用上限（過小な予約による上限突破を防止）
}

enum ReservationOutcome {
    Reserved(ReservationId),
    DeniedByCap(CapStateRef),
    HeldForUnknownCost,              // 適用 cap window の費用が不明で安全に継続できないため保留
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

- 利用実績の元データは各利用担当者（推論、作業、実行、拡張、保全、消去）が保持し、権限・制約担当は利用枠の上限可否のみを管理します。日額のみ・月額のみ・capなしを認め、呼び出し側が単一の`CapId`を選んで他の適用capや復元前cap由来の安全条件を迂回しないようにします。永続化は永続化グループF（`usage_fact_*`）のトランザクションで行い、予約のコミットは短いトランザクション内の不可分な照合（SD-Cap）で行います。予約完了後の推論処理はロックなしで並行実行されます。provider/system 日額・月額 cap の UTC window と dispatch 条件、および復元欠損期間に重なる適用 cap window の費用保留は [Usage / Cost / Cap](usage-cost-cap.md) に従います。復元後にcapを追加・変更したりprovider IDを替えたりしても、新たに適用されるwindowとgapの照合を同じ境界で行い、未照合ならholdまたは変更拒否とします。復元前capのOwner解除は復元済みcapの費用unknownを消さず、予約・作用の`Unknown`もゼロにしません。費用の計算アルゴリズムと他のリソース枠の集計方式は実装の自由度です。
- `CommitUsageCommand` は、システムの再起動後やエージェント停止後の再評価フローからも安全に呼び出せます。処理中にプロセスが停止して宙に浮いた予約（`Reserved`）は、元の予約記録に基づいて「成否不明」として安全に確定させ、勝手に解放（release）してはなりません（PR §6.2、CCT §9.2）。呼び出し元プロセスの生存を必須条件とせず、他者に利用事実の書き換え権限を勝手に移しません。不明となった理由と金額は、管理画面で明確に区別して表示します。
- 予約後に dispatch window が失効、または全適用 cap の現在 revision / amount・残枠、または適用 cap window の現在費用・復元欠損期間 / restore cost hold が送信を許さなくなった場合の `ReleaseUsageCommand` は、transport が provider 0 byte と旧 ticket の送信不能を証明し、未送信試行の記録と `Released` を同じ短い commit で確定するときだけ受理します。cap update と first byte は短い境界で直列化し、既に開始済みなら停止はベストエフォートです。crash 後も `Reserved` のままなら送信前だったと推測して release せず、元 window の `CommittedUnknown` として扱います。次の送信は新しい attempt / reservation と現在の全条件の再照合が必要です。

### K-H Action 候補 → 認可 → 実作用 → 結果確定

権限判定時に記述された対象の名前と、実行時に解決された実際の対象が文字列として一致していることだけで、同一の対象とみなしてはなりません。実行時の厳密な名前解決を経て、真の対象との対応関係を照合します。

```rust
struct ActionCandidate {
    principal_chain: PrincipalChainRef, // 実行主体および委任の連鎖情報
    association: ActionAssociation,  // TaskBound または Direct の閉じた二者択一
    purpose: ActionPurposeRef,       // ユーザー依頼、自発的提案、スケジュール実行、方針変更のどれに基づくか
    described_target: DescribedTargetRef, // 判定時に想定した対象の説明
    operation: OperationKind,        // 読み取り／作成／編集／削除／実行の各操作を混同しない
    workspace_write_purpose: Option<WorkspaceWritePurpose>, // TaskBound Workspace Create/Edit では必須
    save_target: Option<OwnerConfirmedSaveTargetPremise>, // FinalDeliverable の場合のみ必須
    data_use: DataUseRef,            // 取り扱う主なデータと利用用途（認証情報の秘密値は含めない）
    cost_risk: CostRiskRef,          // 予想費用とリスク（推定値か不明かの区別付き）
    relied_intent_rule: ReliedIntentRuleRef, // 依拠したオーナーの意図やルールの対応関係
}

enum WorkspaceWritePurpose { IntermediateWork, FinalDeliverable }

enum ActionAssociation {
    TaskBound { task: TaskRef, scope: TaskBoundActionScope },
    Direct { instruction: OwnerInstructionSourceRef, companion: Option<CompanionId>, target_scope: AuthorizedTargetScopeRef, system_operation: Option<SystemActionOperation>, manual_backup_point: Option<BackupPointId> },
}

enum OwnerInstructionSourceRef { Conversation(MessageId), Management(ActivityId) }
enum SystemActionOperation { ManualBackupExport, CharacterPackageImport, CharacterExport }
enum TaskBoundActionScope {
    Workspace { delegation: DelegationId, workspace: WorkspaceAssocId },
    BackupTaskExport { source: BackupTaskSource, backup_point: BackupPointId, output_scope: BackupOutputScopeRef, setting: BackupSettingRef, setting_revision: BackupSettingRevision },
}
struct AuthorizedTargetScopeRef(RawId);

// Direct は認証済み Owner 入力の source identity と操作対象の許可範囲を持つ。
// TaskRef・delegation・task workspace_assoc を持たず、架空の委任に写さない。

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
    // 消去条件は AU5 / 作用開始 / 結果受入の各境界で current store から照合する。
    // 過去に取得した ErasureConditionRef を生きた許可として渡さない。
    restore_premise: RestoreGeneration, // バックアップ復元を跨ぐ参照を防ぐ世代タグ
}

enum ActionStartOutcome {
    StartedAsAttempt(ActionAttemptId),
    ExistingManualBackupAttempt { attempt: ActionAttemptId, certainty: ActionCertainty }, // 同じ管理 source の受付再送。作用を開始しない
    Denied(DenyReasonRef),
    AskOwner(OwnerQuestionRef),      // オーナーの指示が必要なため実行せず待機
    StalePremise(StalePremiseRef),   // タスクリビジョン、委任、ワークスペース、実対象、許可、帰属、復元の条件不一致
    DataUseHeld,                     // 現在の消去条件・完了後の由来 cut により開始不可（試行・作用なし）
    TaskTerminal,                    // 対象 Task が terminal（Completed / Failed / Cancelled。H-A の cancel を含む）のため開始しない。書き込み・実行ともになし。
                                      // Action 側は Task lifecycle の値型を import せず、Task owner が自分の
                                      // TaskTerminal { task, progress } として意味を説明する
    ExecutionSealed,                 // AU5: delegation の final result が確定済み。書き込み・実行ともになし。
                                      // TaskTerminal と別判定。Action 側は Task lifecycle の値型を import しない
    HeldByGlobalHold(HoldConditionRef), // 安全保留スライスで導入
    CompanionUnavailable,            // Stopped / Deleted / deletion hold。試行・作用なし
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
  ユーザー入力の受付、タスクの受理、認可の判定、外部作用の開始、実際の作用の把握、内部データへの保存、タスクの達成判定、オーナーへの報告完了は、すべて独立した別の事実です。これらを単一の「成功」という状態にひとまとめにしてはなりません。「確認済み成功」「確認済み失敗」「成否不明」を明確に区別し、確認が取れない事象を勝手に「未実行」や「失敗」「成功」へ改ざんしてはなりません。新しい外部作用として許された再実行は独立した新しい試行とし、同じ要求の受付再送は既存試行を読み戻します。Backup 専用 Task の同じ TaskId・手動管理 backup の同じ管理 source から新しい出力試行を作りません。
- タイムアウトが発生しても、外部作用の確信度は変化しません（`Unknown` のまま保持）。遅れて成功の確認が取れた場合は、元の試行記録に対して `Unknown` → `Confirmed` のアトミック更新（CAS）を行い、現在のタスクへ結果を採用するかどうかは H-A の受付インターフェースにおいて改めて判定します。
- 永続化は永続化グループE（`action_attempt`）のトランザクションで行い、実行開始前の比較照合は短いトランザクション内で不可分に完了します（CCT §8.1、PR AU5）。
- Backup 専用 Task の `request_action` は Agent を作らない閉じた producer とし、`TaskBound { scope: BackupTaskExport }` に保存済みの Started occurrence または Owner Run now の由来 / TaskRef / point / 保全 owner の確定出力 scope / Owner 選択の `backup_setting` revision を渡します。AU5 と作用直前に source と Task 種別・非 terminal revision・保存済み source / purpose・担当 Running Companion・Run now 元操作の認証済み帰属・委任不存在・point の出力準備完了と回復不能な失敗 marker の不存在・保存先と保持した実対象・権限 single-use binding・当該 Task の既存 backup 出力試行なしを照合します。`delegation` / `workspace_assoc` も管理起点 `Direct` source も合成しません。別 point での再出力も同じ Task では拒否します。
- `request_action` の Workspace `TaskBound` は AU5 と同じ不分区間で依拠 Task revision・現在 progress 非 terminal・delegation の `task_result` 行の不存在を照合します。terminal は `TaskTerminal`、Task が非 terminal でも seal 済みなら `ExecutionSealed` として試行・作用なしで返し、revision 等の前提不一致は `StalePremise` として区別します。`Direct` は Task / delegation を要求せず、認証済み Owner 指示 source の現在性・role・許可された操作・用途・実対象範囲を同じ Action owner の AU5 で照合します。会話起点なら最新 accepted Owner input と `Some(companion)`、第一者管理起点なら当該認証済み操作の現在性を用います。管理起点の `None` は列挙した system operation に限り、Companion lifecycle / Stop は適用せず、Task・delegation・Companion を合成しません。手動 backup は保全 owner が確定した出力内容と Owner が指定した保存先の実対象、import は Character owner の検証より前に保持した package 原本の実対象と Read 結果、export は検証済み候補・保存先をそれぞれ用途に束縛します。いずれも Permission owner の single-use 評価、AU5・作用直前の現在条件、削除・復元保留、結果保存と `Unknown` の追跡を通します。許可・実対象・費用・保留・消去・復元の gate と試行の確定度・遅延結果の帰属は両者に共通です。manual Computer Use も該当する `Direct` / `TaskBound` の同じ AU5 / Action attempt と Client 側の発行 fence を通し、入力や Client 応答だけから成功を合成しません。
- 手動管理バックアップの `request_action` は保全 owner が一管理 source に確定した唯一の point を `manual_backup_point` に渡します。AU5 は同じ SQLite master で source / point と当該 point の出力準備完了・回復不能な失敗 marker の不存在・出力内容と範囲、現在許可、保持実対象を比較し、その source に既存の出力試行がない場合だけ一件を commit します。受付 retry は同一 source / point と保存済み試行の相関が一致する場合に `ExistingManualBackupAttempt` と現在の確定度を返すだけで、新しい評価・point・試行や作用を発行しません。相関が異なれば fail closed です。最終作用 gate でも同じ source / point の準備完了・失敗 marker 不在と対象・許可を比較します。既存 `Unknown` は再出力せず、未開始の客観的証明がない限り無作用失敗に変更しません。新たな Owner 明示要求は別 source / 新 point で二重出力リスクと現在許可を評価し、旧 `Unknown` を残します。
- **Companion Stop の admission**: AU14 claim と AU5 start（`TaskBound` / `Direct { companion: Some(_) }`）は、担当 Companion の canonical lifecycle が `Running` で削除開始の新規活動禁止 hold がないことを、Stop の `Stopped` commit / hold 確定と同じ SQLite master の短い `Immediate` 比較点で照合します。Stop / hold が先なら `CompanionUnavailable`（推論側は独立した pre-send domain refusal）で attempt も provider 送信も作用も 0。claim / start が先なら attempt とその `Unknown`・利用実績・結果相関を保持し停止は best-effort です。claim 後の provider first-byte と AU5 後の実作用開始でも lifecycle / hold を削除条件と同じ最終 gate で再照合します。`Direct { companion: None }` は管理操作の現在性・許可用途と削除 / 復元等の保留を照合し、Companion Stop の条件を持ちません。Task `Cancelled` の非 terminal gate と混同せず、ローカル停止 token・古い Running read を権威にしません。
- **Workspace Action の消去照合**: `Create / Edit` の Action owner が実際に書く確定済み本文全体（source のない生成本文を含む）を非永続で保持し、既知の canonical source lineage と保持した実対象の erasure coverage、完了後 provenance cut を AU5 の試行挿入と同じ SQLite master の短い `Immediate` 比較点で照合します。重い本文準備・mechanical scan・ファイル I/O は比較点の外で行い、事前評価を同じ本文・操作・実対象・source と active condition set の generation / phase に束縛します。変化したなら比較点外で再スキャンするか `DataUseHeld` で fail closed とし、本文の hash・検索 material・本文そのものを attempt・監査・retry に複製しません。`Read / List` は対象と既知 source を同じ AU5 gate にかけ、取得本文・列挙名を返却・利用・再保存する直前に current mechanical condition / provenance cut と照合します。covered / 判定不能は権限拒否、task stale、技術的エラーと別の `DataUseHeld` として、未開始なら試行 0・作用 0 です（[Targeted Deletion lifecycle §7.1](targeted-deletion-lifecycle.md#71-論理本文と完了後の由来バリア)）。
- **AU5 後の消去競合**: durable attempt は already-started fact ですが後日の作用許可ではありません。削除開始と直列化した短い同期区間で、作用を開始する直前に同じ本文・source・保持した実対象を再照合し、比較と作用開始の間に削除が割り込まないようにします。区間を保持したまま外部 I/O 完了を待たず、開始を確定できなければ区間を解放して再照合します。covered / 判定不能ならその作用を止め、既存 attempt の `Unknown` は無作用を客観的に証明できた場合に限って更新します。結果本文の再保存は owner の受入確定時に current condition / provenance cut を再照合し、旧由来・covered / 判定不能なら本文を残さず試行の body-free な事実だけを保持します。
- **AU5 後の最終作用 gate**: durable な `action_attempt` は実対象作用の開始許可を固定しません。Workspace TaskBound は同じ Task の非 terminal progress、依拠 current revision、delegation 未 seal、現在の Workspace / scope と保持した実対象・操作の同一性を再比較します。Direct は Task を捏造せず、認証済み Owner 指示 source の現在性、`Some` なら同じ Companion、`None` なら管理起点の許可済み system operation と同じ用途・操作、許可対象 scope と保持した実対象を再比較します。双方とも single-use 評価 ID の AU5 で消費済みの binding と依拠 rule / revision（導入済みの場合）、現在の権限・端末・費用 / 保留、削除・復元条件を作用開始点と短く直列化し、Companion Running / Stop は両 TaskBound 用途と Direct の `Some` のみに適用します。Cancel・権限失効・該当する Stop・削除開始が先なら開始しません。未開始を客観的に証明できるときだけ元 attempt を `ConfirmedFailure` + `RefusedBeforeEffect` に settlement し、開始した可能性が残れば `Unknown` を保持して自動再実行しません。開始済み作用は best-effort 停止であり、Cancel を作用取消しの証拠にしません。Computer Use の Host→Client の作用開始点・発行 fence は IPC 契約と接続します。作用完了まで DB transaction / Task-wide lock を保持しません。
- **Client に分散した作用の開始境界**: Computer Use では作用開始点を Host の `ClientActionCommand` の不可逆 transport dispatch に置き、上記 TaskBound / Direct・評価 binding・Stop・削除・消去の最終比較と同じ短い区間で直列化します（HCIPC §15.2）。Stop が先なら未送信 command を 0 byte とし、dispatch が先なら Client が後から入力を始め得る既開始の試行として停止を best-effort にします。Client の物理入力と Host の Stop commit は原子的には比較できず、Stop 通知を受けるまでの in-flight は元試行へ帰属し、証拠なしで無作用や停止完了にしません。
- **本スライス（Task Agent の Workspace 内ファイルシステム限定）の具体契約**:
  - **操作と request/result**: `OperationKind` は `List / Read / Create / Edit` の closed world。request の target は Workspace 相対パスのみとし、絶対パス入力は拒否します。`List` は存在する directory のみを対象とする非再帰列挙で、request は相対 dir path、result は `{name, kind: file | dir}` の列（名前昇順）です。各 direct child は no-follow metadata（`symlink_metadata` 相当）で分類し、symlink / reparse point / junction / mount entry を自動 follow しません。これらは `file | dir` に写像せず結果から除外します（存在しても List 全体は拒否しません）。`Read` は存在する regular file のみを対象とし、request は相対 file path、result は file content です。`Create` は存在しない regular file の新規作成のみとし、request は相対 file path＋body、result は作成済み marker（解決済み実対象付き）です。`Edit` は存在する regular file の上書きのみとし、request は相対 file path＋body、result は更新済み marker です。directory に対する `Read`/`Create`/`Edit`、regular file に対する `List` は拒否します。`Delete / Execute` はユーザー確認・外部拡張の producer を持つスライスが同じ設計変更で追加します。実装は List の no-follow 分類・除外に対する focused test を必須とします。
  - **authorization**: 開始の直前に K-B.1 の `authorize_action_use` が今回限りの判断を返し、`AllowForThisUse` の評価 ID を consume してその opaque な `RawId` に落とし、`AttemptCommitPremise.relied_evaluation` として AU5 の挿入へ渡します。`workspace_assoc` は作業場所の authority であり、それ自体を特定の操作の許可とみなしません。委任の `scope_copy` は依拠時点の写し（provenance）であり、開始時に照合する生きた境界は現在の `workspace_assoc` です。
  - **中間ファイルと最終成果物**: `ActionCandidate.workspace_write_purpose` は TaskBound Workspace `Create / Edit` の閉じた用途で、Action command から K-B.1 の `ActionUseCandidate.write_purpose` と AU5 の attempt 前提へ同じ値を渡します。省略・用途不一致・分類不能は許可しません。中間ファイルは Workspace 内の新規作業ファイルまたは当該 Task が作成した作業ファイルに限り、原本資料や Owner が持ち込んだ既存ファイルを `Edit` で上書きしません。最終成果物（例: `report.md`）は名前や Workspace 内という事実だけで中間ファイルに格下げできず、Owner 確認済み保存先の実対象へだけ公開します。`save_target = None` なら AU5 と実作用直前の双方で最終成果物の外部書込を拒否・確認待ちにします。保存先を後で選ぶ場合は前述の独立した現在 ref / CAS producer が確定してから新しい評価・試行を行い、既存の `workspace_assoc` は不変です。作業終了時の中間ファイル整理はそのファイルの帰属・作用確定度を検証し、削除の Owner 確認・権限 producer が成立した境界でのみ行います。Delete 未導入時に成功を装った自動 cleanup や元資料の上書きで代替しません。
  - **mount / reparse boundary**: 実際の対象の解決（`RealTargetRef`）は実行直前に行い、入力文字列の一致を同一性の根拠にしません。canonical 化とフォルダ配下の prefix 照合に加え、Workspace root の実体境界から外れる target は resolve も start もできません。Linux では `/proc/self/mountinfo` 上の入れ子 mount point を target の祖先に持つ場合と、root と target の device が異なる場合を拒否します。その他の Unix では root と target の device 一致を必須とします。Windows では root と target の volume serial number の一致を必須とします。mountinfo・device・volume の取得を含め、境界の判定ができない場合は fail closed（拒否）します。symlink / reparse point / junction は canonical 解決で外へ出るものを拒否します。`Create` の非存在 target 自体は canonicalize/stat/volume 判定できないため、存在する親 directory を canonicalize し、親と全 existing ancestor に Workspace containment＋symlink/reparse/mount/device/volume の fail-closed 照合を行い、final component が Normal な単一 leaf で未存在であることを確認して `canonical_parent.join(leaf)` を `RealTargetRef` とします。親が存在しない場合・leaf が Normal な単一 leaf でない場合・leaf が既存の場合は拒否します。publish 時の no-clobber 再確認は canonical contract に残します。実装は symlink・mount・reparse・junction の escape 拒否と `Create` の親基準解決に対する focused tests を必須とし、判定を実装できない platform・path は fail closed とします。
  - **解決と使用の結合**: `RealTargetRef` は永続記録する対象の記述であり、実行権限を持つ path ではありません。実行・拡張担当は検証済み Workspace root から対象までの非永続な handle / descriptor 等を保持し、`List` / `Read` / `Edit` はその実対象に対して操作します。`Create` は検証済み親 directory の handle と単一 leaf を保持し、AU5 の後にその親を基準として no-follow・no-clobber で作成します。認可から AU5、実際の open / read / write までの間に名前だけを再解決して使うことは禁じます。対象の同一性や Workspace の実体境界を競合下で保てない platform / path では、読み取りも含め作用を拒否します。AU5 後に保持対象が失われた場合は開始済み試行の事実を残し、作用なしを証明できた場合のみ `ConfirmedFailure`、それ以外は `Unknown` とします。
  - **effect facts**: 作用の確定度は実行の観測（write 後の読み戻し、List の列挙結果）からのみ記録し、エージェントの自己申告を根拠にしません。結果採用・タスク達成はこのスライスに含めません（H-A の受付インターフェースが担当）。`orchestrate_workspace_action` は AU5 の `DataUseHeld` を `ActionNotStarted` の独立した data-use hold として返し、開始済み試行が後続の消去照合で止まった場合は `ActionRunOutcome` で試行 identity と確定度を維持します。既存試行の消去停止を「試行なし」に写しません。

### K-I 拡張受入

```rust
struct RequestExtensionUseCommand {
    extension: ExtensionKind,        // MCP Tool／Resource／Prompt、プラグイン（限定された拡張点）
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
// MCP の結果やプラグインの出力、Agent Skills の内容は、公式（第一者）の制御変更や権限承認を直接成立させません。
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
- **ホスト再起動時**: 在席状態の復旧と、中断された対話ラウンド・Computer Use・タスク・アクションの再実行・再開は、まったく別の条件に基づきます。実行中（Running）だったパートナーは再起動前のクライアントへ安全に復帰を試みますが（元 Client が未接続なら PR §6.4 の RecoveryWait を保持する）、再起動で中断されたタスクはオーナーによる明示的な再開指示を待ちます（cancel された Task は再開せず、再実行は新しい Task の下に新しい delegation を作成して行います）。失われるのはローカルの Future・cancellation token・停止 handle だけであり、開始済みの provider 処理・外部作用が停止したとは推定せず、durable な attempt と `Unknown`、遅延結果の帰属を維持します。ホスト停止中に到来したスケジュール実行はスキップ（Missed）扱いとし、未完了のデータ削除やバックアップ復元の安全保留は確実に維持します。

### X-H 未伝達の次 Client 報告

H-G による未伝達情報の登録に加え、新しいクライアント側でその要約を提示するためのインターフェースです。

```rust
struct RequestUndeliveredSummaryQuery {
    companion: CompanionId,
    to_client: ClientId,
    presence: PresenceCheckRef,      // 新しい在席が正式に成立した後の、現在の在席帰属
    cursor: Option<UndeliveredCursor>,
    limit: u32,                     // 1..=50。SQL 側でも適用
}

struct UndeliveredSummaryFact {
    items: Vec<UndeliveredRef>,      // 元のタスク記録や活動記録への対応関係付き
    current_filter: CurrentFilterRef, // 最新の処理結果、利用制限、データ削除状況との照合フィルター
    next_cursor: Option<UndeliveredCursor>,
}
// 要約の提示途中でクライアントが再び切断された場合は、提示状態を「成否不明」として保持します。
// 厳密な1回のみ配送（exactly-once）や、ユーザーが確実に読んだことの保証などを無理に追加しません。
```

query は read-only です。入出力・提示が結果を会話タイムラインのテキストと音声向けに整形し、現在の connection / presence と対応する新 Round・receipt を発行します。`begin_presentation(receipt)` が選択行の提示開始を commit してから送信します。Task report は既存の facts の定型表示を基本とし、再接続のためだけに provider を呼びません。Companion の表示人格を保つ文面でも Task の certainty を強めず、LLM が unavailable でも進捗・結果・resume 操作へ到達できます。通常の Task 操作に一覧選択や専用 report 画面を要求せず、詳細記録の read query は診断・成果物確認・安全操作のために残します。

receipt の内容は Host が保持する `(connection, incarnation, companion, client, round, generation, selected_ids)` です。Client による id 集合や帰属の組み替えは拒否します。`confirm_presentation(receipt, status)` は [CCT §10.5](concurrency-control.md#105-未伝達-ack-は選択した事項だけを確定する) の比較後、個体調整の report state を更新します。通常返信の既存 `ConfirmPresentation` も、その返信に対応する同じ receipt 境界を通します。元 Round の provenance と再提示用の新 Round は別に保持し、古い入力を再開しません。

`Presented` にできるのは、選択した各事項について、その receipt に載せた要約本文全体が会話タイムラインに実際に表示されたか、音声として最後まで再生された場合だけです。読み上げが途中停止・失敗してもテキスト提示が成立すれば同じ事項は提示済みです。生成・送信・部分表示・音声の一部出力は提示済みの証拠になりません。batch の一部だけが提示された場合、未提示の事項を同じ ACK で Presented にしません。提示は既読、理解、正式な許可、Task 成功ではありません。

ACK 待ちに切断・replacement・Host crash が起きたら durable な PresentationUnknown が残ります。再接続では Pending と Unknown を新 receipt で再提示できます。Presented はユーザーの既読・承認でも Task 成功でもありません。失敗・Unknown が同じ pass の自動送信を繰り返すことはなく、後続ページを先に扱います。通常の Task report query は表示だけで未伝達を消さず、表示した事項を消し込む UI はこの ACK を明示的に送ります。

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
struct AppliedCurrentState {
    version: AppliedCurrentVersion,         // Companion ごとの適用関係の版。初期の未適用状態にも版を持たせる
    applied: Option<AppliedPartListRef>,     // None は未適用
}

struct ProposeCharacterApplicationCandidate {
    companion: CompanionId,
    character: CharacterId,
    expected_character_revision: CharacterRevision, // 期待される確定済みの静的定義リビジョン
    expected_applied_current: AppliedCurrentState, // オーナーが選択した時点の個体の適用状態
    selected_parts: Vec<CharacterPart>,  // オーナーが部品ごとに明示的に選択した更新範囲
    owner_selection: OwnerSelectionRef, // 管理画面の選択操作 identity と選択内容の対応関係
}

enum CharacterApplicationOutcome {
    AppliedAs(AppliedCurrentState),   // 個体調整担当が適用関係を確定（パートナーが積んできた経験や記憶は初期化しない）
    StaleApplicationPremise { current_applied: AppliedCurrentState },
    DataUseHeld,                      // 適用内容・由来が現行の消去条件に覆われるか判定不能
    DeniedProhibitedPart(ProhibitedPartRef), // 認証情報、権限昇格、同意の代用など、適用が禁止されている危険な部品種別
    NeedsOwnerSelection,             // 部品ごとの明示的な選択が不足しているため確認が必要
}
// パッケージの更新によって、パートナーのこれまでの成長や思い出を勝手に初期化してはなりません。
// また、オーナーが確認していない部品を勝手に更新済みに変えてはなりません。
```

- `expected_applied_current` は個体調整担当の現在適用関係を読んだ時点の premise です。`version` は同じ Companion の適用が確定するたびに前進し、未適用状態や再適用後も以前の値を再利用しません。`compare_and_set_applied` は同一 SQLite master の短い DB 比較点で、確定済みの `expected_character_revision` と選択部品・資材、`expected_applied_current` と現在の `companion_applied_current`、`owner_selection` の操作 identity とその Companion・選択部品・両 premise への対応を照合します。適用する静的本文・管理下資材の内容と埋込情報・参照・既知 source lineage の機械的消去 coverage と完了後 provenance cut は候補の同一性と現在の condition generation / phase に束縛して比較点の外で評価し、同じ master の適用 commit では候補と current condition / phase・cut を削除開始・finalizing・completion と直列化して再照合します。削除が先で被覆あり・由来不明・事前判定が古い場合は `DataUseHeld` として適用・履歴追加をせず、重い再評価は比較点の外で行います。一致した場合だけ、個体調整担当の適用関係・版の前進と履歴追加を不可分に確定します。同じ操作 identity を異なる選択内容や premise に付け替えず、二重要求で二度適用しません。定義 revision または適用 current が変わっていたら `StaleApplicationPremise` で現行状態（未適用も含む）を返し、現行の差分を示してオーナーに再選択を求めます。適用・履歴追加は行いません。
- 永続化は永続化グループA/B（`character_revision` ＋ `companion_applied_current` ＋ 変更履歴）で行い、コミットは同期区分 SD-CharApply の短いトランザクション内で不可分に完了します（CCT §4）。参照中の改訂・部品と管理下アセットは通常整理や個体削除の際にも保持します。個体の経験・記憶はこの更新に含めません。

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
}

struct CharacterExportFact {
    export_ref: ExportRef,           // 外部配布用のコピーファイル（内部所有権ではなく単なる成果物）
    included_scope: IncludedStaticScopeRef,
}

enum CharacterExportOutcome {
    Exported(CharacterExportFact),
    RejectedUnsafeContent,
    RejectedIncompletePackage,
    StaleRevision,
    DataUseHeld,                     // 現在の消去条件・完了後 cut・由来判定不能による公開保留
}
```

Character owner は静的構成を候補として、実際に出力する本文・ファイル名や manifest 等のメタデータ・管理下資材の内容と埋め込み情報・由来を配布可能性の判定へ通します。特定ユーザーの個人情報や秘密値の非混入を確かめられない候補は除外または `RejectedUnsafeContent` とし、除外後の参照・必須要素が成立しなければ `RejectedIncompletePackage` とします。経験・記憶・会話・認証情報・権限は候補にも含めません。静的という分類やオーナーの内容確認は除去証明ではありません。Character owner は確定候補・検証済み出力範囲・由来を Action owner へ渡して外部公開を依頼し、外部ファイル操作や許可・試行の確定を自ら行いません。公開前の拒否は `CharacterExportFact` を作りません（CD §CD-5、CD §7.2）。

配布候補の検証と公開の間には Targeted Deletion が開始し得ます。Character owner は確定済みパッケージ全体（人格・スキルの本文、manifest / ファイル名 / 参照先、管理下 asset の内容・埋込情報と由来）の機械的消去 coverage、既知 source lineage、完了後 provenance cut を transaction 外で評価し、候補・`CharacterRevision`・消去条件の generation / phase に束縛します。PII の静的分類、形式検証、目視確認では代用しません。Action owner は Permission owner の今回限りの評価と外部パスの実対象を照合し、実際に公開する同じ内容・由来・対象を、帰属に応じた `TaskBound` または `Direct` の AU5 と作用直前ゲートで現在の revision・候補の同一性・condition / phase / generation・cut に再照合します。TaskBound は Task / delegation / Workspace と必要な確認済み保存先、Direct の Companion 付き経路は認証済み Owner 指示・同じ Running Companion・許可対象範囲を要します。個体に依存しない管理画面からの公開は `Direct { instruction: Management(_), companion: None, system_operation: Some(CharacterExport) }` とし、管理操作の現在性・Owner が選んだ出力先・用途・操作・実対象を評価と AU5 に束縛します。候補の検証や stage の存在を許可とみなしません。削除開始が先なら旧候補は `DataUseHeld` として公開せず、条件変化時の再 scan は transaction 外で行います。AU5 が先でも試行は公開許可や成功の証拠ではなく、作用直前に削除開始・失効が先なら停止します。Action owner は不可逆な公開開始を deletion start と同じ SQLite master の短い同期区間で直列化し、区間内で開始できなければ解放して再照合します。外部 I/O 完了まで DB transaction を保持しません。公開済みの外部コピーは Ene の消去管理外ですが、公開前 staging は削除 participant の列挙・局所消去・残存検証に含め、完了後も旧 stage の後出し公開を cut / lineage で拒否します。Action owner が確認した公開成功だけを `Exported` に反映し、未開始の拒否と開始済み試行の `Unknown` を混同せず、途中失敗・結果不明を成功や自動再実行へ写しません。公開先は実対象を保った no-clobber / atomic publication と単一 writer / lock / CAS により保護します（[K-H](#k-h-action-候補--認可--実作用--結果確定)、[Targeted Deletion lifecycle §7.2](targeted-deletion-lifecycle.md#72-character-export-公開と削除の競合)）。

## 8. Data preservation / lifecycle クラスタの interface semantics（D-A〜D-E）

個人データ完全削除（Targeted Deletion）を、単なる汎用の `delete(entity_id)` のような安易な関数で表現してはなりません。また、調整役（coordinator）が各担当ドメインの内部状態を勝手に直接書き換えるようなインターフェースを作ってはなりません。

### D-A Targeted Deletion の開始・範囲確定

```rust
struct RequestTargetedDeletionCommand {
    purpose: DeletionPurposeRef,     // プライバシー保護やセキュリティ上の削除目的を明示（通常の忘却処理等との混同を防止）
    target: TargetedDeletionTarget,  // LLM に依存しない厳密な機械的条件（必須レイヤー）
    exclusion: ExclusionRef,         // 削除から除外すべき対象や、事前確認が必要な範囲の対応関係
    impact_notice: ImpactNoticeNeedRef, // 削除の目的、対象範囲、影響度、除外対象に関するオーナー向け事前説明用データ
}

struct TargetedDeletionTarget {
    mechanical: MechanicalDeletionTarget,
    semantic_hints: Vec<SemanticDeletionHint>, // 探索補助。機械的条件を代替しない
}

enum MechanicalDeletionTarget {
    ExactText(DeletionSearchMaterial),
    RegisteredCredential { r#ref: CredentialRef },
}

struct DeletionScopeDecision {
    operation: DeletionOperationId,
    sweep: DeletionSweepGeneration,  // 削除処理区間の厳密な順序世代
    valid_interval: ErasureInterval, // 削除処理の開始から検証完了までの有効期間（区間内に遅延到着したデータの取り込みを含む）
    participants: Vec<ParticipantOwnerRef>, // 消去処理に参加すべき全領域（オーナーに保存場所を手動選択させない）
}
```

- `ExactText` は指定文字列、`RegisteredCredential` は Owner が選んだ非秘密 `CredentialRef` の対象 version 値を認証秘密 owner が OS secure store 内で限定的に解決した条件です。両 variant とも内部全域の機械的検索・消去・残存検証が必要です。`ExactText` が登録値に一致した場合も owner が照合し、漏洩コピーのみか登録抹消かを区別します。同値の複数登録は用途・接続影響・再認証とともに対象範囲を確定し、除外登録が残存 0 を妨げるなら再確認します。値・hash・検索 token を通常 SQLite や Client DTO へ出しません。LLM の結果を検証の代わりにしません。
- 上記の `DeletionSearchMaterial` は Host 内部の protected material であり、wire の通常 request フィールドではありません。remote `RegisteredCredential` は非秘密 ref を通常の第一者削除 DTO で選べます。remote `ExactText` は照合前から登録秘密と一致し得るため、値を第一者削除専用の短寿命 secret-bearing input frame で credential owner に渡し、通常の management / chat / deletion preview / completion DTO、log、Client 長期 cache に載せません。owner が同値登録を照合した後も対象の非秘密参照と一時 handle で確認を束縛します。限定入力境界を提供できなければ remote 値指定を拒否して登録対象の ref 指定へ誘導し、Host GUI が無い remote ref 指定は維持します。通常の credential 登録・更新の Host-local 境界には転用しません。
- `RequestTargetedDeletionCommand` は確認すべき範囲と影響の候補であり、受付だけで operation や消去条件を開始しません。`DeletionScopeDecision` は確認と durable な開始が確定した後の事実だけを返します。実際の開始は TDL §4 の `StartTargetedDeletionCommand` が target variant・対象 version / 登録範囲・目的・影響・expected currentness に束縛された一回限りの `TrustedOwnerConfirmationRef` を持ち、保全・消去担当が同じ短い確定点で現在の対象と確認由来を照合した場合に限ります。Host-local の専用面は Host が由来を検証します。remote では、信頼する公式 Client が専用面で Owner の直接入力を受け、Host は認証済み接続・session・incarnation と一回限りの challenge・現在性を検証して Client の確認報告を受理します。Host は Client 上の人の物理入力そのものを証明できず、Client process / OS / 公式コードと対話 session の完全性を前提にします。通常のリモート intent、`confirmed=true`、LLM や Computer Use の結果を確認としません。Host GUI 不在でも正規の Client 専用確認を拒否せず、API キー登録・端末ペアリング・リストア・全リセットの Host-local 確認をこの経路で代用しません。確認不能なら削除開始前の refusal / hold として消去条件を書きません。
- 消去条件の durable 化（durable-before-enforce）は、各利用・送信箇所（少なくとも Task Agent 推論試行 claim）が自身の解決済み source 相関を機械的に照合できる形で行います。どの source 相関が現在の条件に覆われているかの意味判断は保全・消去担当が持ち、利用側は coverage の有無だけを判定して削除の意味を解釈しません。canonical current-condition store は Stage 4 erasure-currentness foundation が導入し、Stage 6 の Targeted Deletion producer が同じ store へ active condition を durable-before-enforce で投入します（別の gate / currentness registry / source correlation を作りません）。active condition set が空の場合の「被覆なし」は store を実際に照会した authoritative な結果であり、`ErasureConditionRef::none()` / generation 0 / 「削除なし」marker のような sentinel ではありません。利用側に「削除なし」既定値や派生 marker を置かせず、producer の無い消去前提を捏造させないことを契約とします。
- 登録秘密の operation 用 OS 検証材料は保存前に非秘密 preparation identity / item version を durable 化し、開始 commit で同一材料の参照を operation へ原子的に移す。OS I/O 中は DB lock を保持しない。serving startup の mutating reconciliation は専用 namespace の全候補を列挙し、operation に接続しない準備 material の消去・再照会を確認してから通常 credential 利用と新規 deletion を許可する。通常 read は変更しない。全域の機械検証前に接続済み材料を失ったり、消去成否が不明なら完了とはしない（[TDL §3.1・§4.1](targeted-deletion-lifecycle.md#31-operation-中だけ保持する検索材料)）。

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

- 各データ保持者は、`(元データとの関係, 処理中の利用状況, 局所消去と検証, 再保存防止, 未確認・未完了範囲)` の状態を保全・消去担当へ正確に報告します。元データを消去した後に依存関係まで一緒に失われ、遅延して届いた結果が削除対象かどうか識別できなくなるような実装は禁止します。対象値や hash ではなく、body-free の source / attempt identity と開始時に確定した provenance cut で旧由来を辿ります。

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

登録秘密では全参加者の本文 scan・残存検証、旧 in-flight の収束、`Finalizing` の本文受入停止を終えてから、対象 OS item と独立した OS secure store の一時検証材料を消去・再照会します。材料欠落・破棄成否不明なら pending / unavailable を維持して必要なら Owner に再提供を求め、全域完了を宣言しません。完了前に検索材料を完全消去し、完了後は body-free cut で旧由来を拒否し、新しい Owner 入力は新しい経験として扱います。

### D-D Backup 作成・Restore staging・マスターデータ切り替え・復元後保留・一括有効化

```rust
struct CreateBackupCommand {
    setting: BackupSettingRef,       // 保存先・保護設定。時刻条件と各回の Task は作業の Schedule が所有
    source: BackupRequestSource,
}

enum BackupRequestSource {
    ManualManagement(OwnerInstructionSourceRef),
    TaskBackup { task: TaskRef, source: BackupTaskSource },
}

struct ManualBackupOutput {
    source: ActivityId,
    backup_point: BackupPointId,
    output_scope: BackupOutputScopeRef,
}

struct BackupTaskOutput {
    source: BackupTaskSource,
    task: TaskRef,
    backup_point: BackupPointId,
    output_scope: BackupOutputScopeRef,
    setting: BackupSettingRef,
    setting_revision: BackupSettingRevision,
}

struct BackupPointFact {
    backup_point: BackupPointId,
    covered_scope: CoveredScopeRef,  // 対象時点、参照の整合性、除外されたデータ（秘密値や外部実体）、処理中タスクの未完了状況
    // 各テーブルのコピーが個別に成功したことだけでバックアップ成功とみなしてはなりません。
    // 対象時点、参照関係、変更履歴、未完了状況の整合性が完全に揃って初めて成功と確定します。
}

保全 owner は各担当の整合性を検証した point / output scope と保存先設定の現在値を確定し、Backup 専用 Task 出力なら `BackupTaskOutput` を Host 結合ルートから Action / Permission owner へ渡します。通常 Task の Workspace 試行はバックアップ point の成功・出力の候補に流用しません。外部出力を実行する Action owner の `ConfirmedSuccess` は実対象への作用の証拠であり、`BackupPointFact` の対象時点・参照・除外・未完了状態まで検証したバックアップ成功 marker とは別です。保全 owner は検証済みファイルの永続化と Action の確認済み成功が揃って初めて point を成功とし、Task owner は同じ Backup 専用 Task / source / point の保全成功 marker とその一件の Action の `ConfirmedSuccess` を読み、同じ `Immediate` で current revision・非 terminal・委任不存在と Task-wide `Unknown` barrier を検証して `Completed` を CAS します。`task_result` / `task_result_attempt` や Agent の final result を捏造しません。未完了、`Unknown`、作用済みでも保全未確認のものは hold とし、再起動・復元後は保存済み fact の再評価だけを許し、外部作用を自動再実行しません。

Backup 専用 Task の回復不能な作成失敗は、保全 owner の同じ point に対する検証済み失敗と Action owner の試行確定度・作用根拠を受け、外部作用の影響範囲が特定され未解決の作用も残作業による達成可能性もない場合にだけ Task owner の `fail_task` が扱います。`task_kind` / source / purpose・TaskRef・非 terminal 状態・point・全試行の確定度と Task-wide `Unknown` 不在を同じ master の比較点で確認し、`Failed` と失敗帰属・未伝達を不可分に確定します。試行前の確定した作成不能も無作用と証明できるときに限ります。設定変更で再評価できる場合、技術的障害、影響範囲の不明な部分作用や `Unknown` は成功にも terminal failure にもしません。既知の部分作用は残ったファイル・保存先を含めて報告します。`Failed` の結果と判明した影響を報告し、同じ Task は再開しません（PR §4.5）。

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

定期バックアップは担当 Companion の Schedule の各回から、Run now は認証済み Owner 操作とその Schedule の現在条件から、それぞれ独立の新 Task を作り、現在の権限・ルールを評価した依頼として保全・消去へ渡します。Schedule と無関係な管理操作からの手動作成は Task / Companion を必要としません。どちらも保全・消去が対象範囲と参照整合性・結果を確定します（BR §6、PR §4）。

`ManualManagement` は認証済み `Management(ActivityId)` だけを受け、保全 owner は同じ SQLite master に source 当たり唯一の `ManualBackupOutput` を先に確定します。同じ source の再受付は既存 point / output scope を読み、別 point を発行しません。Host 結合ルートはその source / point / output scope と確定出力内容を `Direct { companion: None, system_operation: Some(ManualBackupExport) }` の Permission / Action に渡します。Action AU5 の当該 source / point 一試行と保全 owner の point 成功 marker は別事実です。試行 `Unknown` のままなら管理経路で未確定と二重出力リスクを提示し、ファイルの存在や再起動を成功・失敗・未開始の証拠にしません。別の出力には Owner の新たな明示管理要求 / 新 point と現在の許可・リスク判断を要します。

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

特に権限の大きい重大な操作の最終確認は、Host が信頼されたインストールから起動した公式 GUI の直接確認に限定します（[要件「信頼境界」](../../requirements/requirements.md#信頼境界)、[Runtime Topology](../architecture/runtime-topology.md#第一者確認面の信頼前提)、IPC §18）。Host はその child にだけ継承した非公開 endpoint と生存記録から exclusive `FirstPartyControlSeat` を発行し、操作・対象・前提と seat に束縛した one-shot `ConfirmationSession` の completion を各 owner の確定境界で照合します。nonce は freshness だけであり、公開 listener への先着、同一 UID、PID、`confirmed=true`、CLI や tool の出力を確認権限にしません。`ene-local-control` は要求専用 DTO と確認専用 DTO を分離し、requester は非秘密 request / outcome だけを扱います。GUI 不在時は Host が起動し、起動できなければ `ConfirmationUnavailable` とします。offline CLI で確認を省略する経路は残しません。通常 Client、`ene-ctl`、LLM、tool、plugin、Computer Use の結果電文は最終確認経路ではなく、ene Computer Use は確認面へ入力できません。Host / GUI restart は未消費 session を失効させます。詳細は [First-party desktop 第5節](first-party-desktop.md#5-trust--credential--failure)。会話の稼働や provider 成功を管理操作の前提にはしません。

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
| H-A タスク化／方針指示／委任 | ● `TaskRef` ＋ ● `task.progress` 非 terminal（委任 AU3・方針指示 AU4。terminal（Completed / Failed / Cancelled）は `TaskTerminal` で書き込みなし） | ○ 復元前提（`RestoreGeneration`。復元跨ぎ参照防止。保全・消去スライスで導入） | ● 委任元パートナー・タスク・ワークスペース | ● 作成時の目的・方針指示前提（`SteeringPremiseRef`）・委任スコープ | ○ `DelegationRef` | —（達成は作業担当、確信度は実行担当） | ● 安全停止・保留・削除・復元保留の照合（hold スライスで導入） |
| H-A タスク中断（Cancel、AU16） | ● `TaskId`（リビジョンは問わない。同時 steering で revision stale にしない。会話 source は同じ Task の中断を後続の認証済み Owner 指示が明示撤回・置換した場合のみ Superseded） | ○ 復元前提（`RestoreGeneration`。復元を跨ぐ要求の失効・hold 中の受理可否は保全・消去スライスが hold と対で同じ比較点に追加） | ● 個体調整（会話）または第一者管理経路 → 作業担当（LLM・エージェントの終了を待たない） | ● 対象タスクと source（会話なら body-free `OwnerTaskCancelIntentRef`（Owner message ID / accepted sequence・role・Companion・対象 Task・直接的中断意図の正本）と元 intent / Task に結ぶ型付き後続 fact、管理なら認証済み操作 source / 対象 Task。曖昧なら確認まで保留。理由本文は持たず履歴・管理経路から報告） | — | —（停止完了は含めない。作用の確定度は Action owner の事実のまま） | ● 受理は progress の `cancelled` への CAS のみ。既存の非 terminal gate（AU3/AU4/AU14/AU5/AU15b）が新規開始・結果採用を拒否。開始済み試行・Unknown は不変 |
| H-A エージェント結果受入 | ● 委任対応（delegation 行）から解決する依拠 `TaskRef` と、現在 `task.revision` の一致 | ○ 復元／消去の各世代タグ（producer スライスで追加） | ● 結果 identity・委任（execution lifetime = 1 delegated Task Agent execution）・依拠 revision。final result の到着で委任を seal | ● 現在の目的 identity との整合性（本文文字列の一致は使わない） | ● 依拠 Action 試行相関（delegation から列挙した authoritative set と claim の完全一致。欠如・追加・重複は技術的エラー。membership は seal 時点で固定）。確定度は owner 事実のまま（完了は result-local の `Unknown` / 非依拠を検証できない失敗と、同じ `TaskId` の全 revision / 全 delegation に残る `Unknown` が無い場合だけ成立。Task-wide completion barrier） | ● 結果本文は Task owner が到着時に 1 行だけ保存（自己申告を確信度にしない） | ● 中断（progress が `Cancelled` = terminal）・方針変更・terminal・execution seal・削除・復元との照合 |
| H-B 経験提出 | ○ タスク由来なら `TaskRef` | ○ 在席／復元／消去の各世代タグ | ● 経験したパートナー・タスク・委任 | ● 想定用途・期待スコープ | — | — | ● 保存禁止・非共有・消去条件の照合 |
| H-C 会話による訂正 | ● `(LearningId, expected_revision)` | ○ 消去スイープタグ | ● 訂正対象・根拠となる新経験 | ● 時間的な意味論（最初から誤り／変化） | — | — | ● 消去・制約条件の照合 |
| H-D 自律学習参照 | ○ `(LearningId, revision)`（取得データ側） | ○ 消去スイープタグ | ● 利用元・目的・要求スコープ | ● 用途・スコープ・制約の写し | — | — | ● スコープ制限・保存禁止・消去保留 |
| H-E スコープ変更 | ● `(LearningId, expected_revision)` | ○ 消去スイープタグ | ● 対象項目・共有内容 | ● 共有内容・背景・非共有意図 | — | — | ● オーナーの明示的制約の照合 |
| H-F ルーティング文脈供給 | ○ 元データとの対応 | ○ 在席／消去の世代タグ | ● 元の担当責任者・対象パートナー・用途 | ● ルーティング用途・制約・選択前提 | ○ 観測候補との対応 | — | ● 利用同意・消去・失効状態の照合 |
| H-G 未伝達メッセージ管理 | ○ 元データとの対応（Task／Activity） | ● 在席世代 ＋ 復元世代 | ● 対象パートナー | ● 提示・報告の進行状況 | ○ 対話ラウンドID | ○ 提示完了／成否不明 | ● データ削除状況の照合 |
| K-B リアルタイム認可照合 | ● タスク／ルール／同意の期待リビジョン | ○ 在席／復元／消去の各世代タグ | ● 実行主体・委任・タスク・ワークスペース | ● 目的・対象・操作・データ・送信先・費用 | ○ 試行前提 | — | ● 認証失効・安全停止・費用上限・保留 |
| K-C 秘密情報利用 | ○ `CredentialRef`（非秘密）＋ 用途 | — | ● 接続先・用途 | ● 認証用途の厳格な制限 | ○ 操作種別 | ○ 認証成否・再認証要否 | ● 認証失効・安全保留の照合 |
| K-D/E/F 推論実行・フォールバック | ○ 同意情報の期待リビジョン | ○ 復元／消去の世代タグ | ● 消費主体・要求能力 | ● 用途・データ・送信先・取扱制限・費用 | ○ チケットID | ○ 能力不足・送信失敗・実測利用量 | ● 保存禁止・費用上限・保留・消去 |
| K-G 利用枠予約・確定 | ● 実課金先・帰属先（適用capはownerが解決） | ○ 復元世代 | ● 消費主体・用途・送信先 | ● 適用capのscope × UTC window・引当予約量（cap未設定も可） | ○ 予約ID | ○ 確定実測値／成否不明／処理中 | ● 復元前cap由来の短命な安全条件・適用capの上限超過／費用不明のみ照合。非適用windowのgapは停止しない |
| K-H アクション実行開始 | ● TaskBound はタスクリビジョン・`task.progress` 非 terminal、Workspace なら execution 非 seal、Backup 専用出力なら保存済み二択の source / point / 保存先の現在性、Direct は認証済み Owner source の現在性・許可 scope（管理起点 None は列挙 system operation も）＋ルール／同意の前提 | ● 在席（該当時）＋ 復元 ＋ 消去 | ● 閉じた帰属：TaskBound は Task と Workspace 委任またはBackup 専用 Task の Started occurrence または Owner Run now source / point / scope / 設定、Direct は Owner source・optional Companion・scope・optional system operation。Workspace TaskBound attempt のみ delegation の authoritative set に入る。Backup 専用出力は Task-wide Unknown barrier に入る | ● 目的・実対象・操作・既知 source、`Create / Edit` の非永続な確定実書込本文・送信先・費用 | ● 今回限りの認可判定ID ＋ 費用予約ID（消去の事前 snapshot は許可ではない） | —（開始時点） | ● AU5 と作用直前の current erasure condition / provenance cut。covered / unknown は独立した `DataUseHeld`、未開始なら試行・作用 0。認証失効・安全停止・費用上限・保留・復元、Companion 付きなら Running / Stop も照合 |
| K-H 外部結果確定 | ● 試行ID ＋ 期待される確信度 | ○ 世代タグ | ● TaskBound のタスクと Workspace 委任または Backup 専用 Task の Started occurrence / Owner Run now source・point、または Direct の Owner source・optional Companion・system operation | ● 対象・外部作用との対応関係 | ● 元となった成否不明試行ID | ● 確認済み成功／確認済み失敗／成否不明 | ● 中断・再実行条件の照合 |
| X-A 在席移動 | ● `(CompanionId, expected_generation+state)` | ● `PresenceGeneration` | ● 対象パートナー・移動元端末・移動先端末 | ● 移動の発生理由 | ○ ラウンド終了対応 | ○ 到達性・排他性の確認 | ● ペアリング・デバイス・安全停止・保留 |
| X-B 対話ラウンド受付 | ○ `(Companion, Client, round)` ＋ 申告世代 | ● `PresenceGeneration` | ● 対象パートナー・端末・ラウンド・観測候補 | ● 対話の目的 | ○ ラウンドID・試行ID | ○ 受付完了／提示完了／成否不明 | ● 端末切り替え・安全停止・保留・消去 |
| X-E 観測結果ルーティング | ○ 候補ID ＋ 文脈との対応 | ○ 在席／消去の世代タグ | ● 取得元端末・対象パートナー・用途 | ● ルーティング用途・配信制約 | ○ 観測候補ID | — | ● 観測制御・利用同意・消去の照合 |
| D-B 局所データ消去 | ● `(operation, sweep)` ＋ 元データとの対応 | ● `DeletionSweepGeneration` | ● 参加担当者・データ保持の対応関係 | ● 削除目的・対象範囲・除外設定 | — | ○ 局所的な検証結果 | ● 有効期間・完了境界・安全保留 |
| D-D バックアップ復元 | ● バックアップID ＋ 世代前提 | ● `RestoreGeneration` | ● 復元対象・参照関係の対応 | ● 用途・再有効化の範囲 | ○ 復元ID／ステージングID | ○ ステージング検証・マスター切り替え成否 | ● 復元後の一括保留・認証情報の照合 |
| C-B キャラクター適用 | ● `(CharacterId, expected_revision)` ＋ `expected_applied_current` | ○ 復元／消去の世代タグ | ● 対象パートナー | ● 選択部品・オーナーの明示選択と操作 identity | — | — | ● 適用禁止部品種別の照合 |

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
| タスク提案・方針指示・委任（コマンドレベル） | `TaskProposalOutcome`、`DelegationOutcome` | 受理（Accepted）／前提不一致（StalePremise：現在値付き）／タスク終端（TaskTerminal：Completed / Failed / Cancelled のため書き込みなし。方針指示・委任時）／タスク未存在（MissingTask：方針指示・委任時）／リビジョン上限超過（RevisionExhausted：方針指示時のみ）／全体保留中（HeldByGlobalHold：hold スライスで追加）／再照合が必要（NeedsRevalidation）／情報不足（InsufficientContext）。TaskProposalOutcome は候補側の意味論をすべて持ち、DelegationOutcome は HeldByGlobalHold・NeedsRevalidation・InsufficientContext を、それぞれの producer を持つスライスで追加します。Cancel は専用の `TaskCancelOutcome` を持ち、この enum に混ぜません |
| タスク中断（Cancel、コマンドレベル・リポジトリレベルで同一） | `TaskCancelOutcome` | 中断受理（CancelAccepted：現在の progress を started / in_progress から cancelled へ不可分に CAS。停止完了や外部作用の確定度は含めない）／再要求（AlreadyCancelled：既に cancelled。書き込みなしの冪等結果）／終端済み（TaskTerminal：Completed / Failed のため中断不可。書き込みなし）／タスク未存在（MissingTask）／会話入力の明示的な置換（Superseded：元 intent / Task に結ぶ後続の認証済み Owner 指示が明示撤回・置換。無関係な後続入力は該当しない。書き込みなし。管理経路には適用しない）／確認待ち（NeedsClarification：後続の未解決の確認待ち。元入力の対象・意図が曖昧なら会話担当は intent を発行せず Owner に確認。書き込みなし）。受理は durable な progress 遷移のみを意味し、実行中の作用の停止完了は Action 試行の確定度と推論試行の durable facts から別途報告します |
| タスクコミット（方針指示 AU4、リポジトリレベル） | `TaskCommitOutcome` | コミット成功（CommittedAs）／期待値不一致（StaleExpected：現在値付き）／タスク終端（TaskTerminal：Completed / Failed / Cancelled のため書き込みなし）／タスク未存在（MissingTask）／リビジョン上限超過（RevisionExhausted）／全体保留中（HeldByGlobalHold：hold スライスで追加） |
| エージェント結果到着（AU15a） | `TaskResultArrivalOutcome` | scrub 証明の revision と現在 credential-set revision の不一致（StaleCredentialSet：現在値付き。本文・seal 書き込みなし）／到着記録・seal（Recorded）／到着時本文保留・seal（ErasureHeld）／事後消去済みの再到着（BodyErased）／旧本文を検証できず移行時に除いた既存 ID（LegacyBodyUnverified）。確定済み ID の retry は帰属を照合し、本文が残る recorded では本文一致を必須として不一致は技術的エラー、body-free の三状態では帰属だけを比較し、外部作用を再実行しない |
| エージェント結果受入 | `TaskResultAcceptance` | 消去による到着時本文保留（ErasureHeld：body-free 到着・seal。採用不可）／保存後の本文消去（BodyErased：未採用なら採用不可、採用済みなら採用事実は保持）／移行前の本文の安全性を証明できず除去済み（LegacyBodyUnverified：未採用なら採用不可、採用済みなら採用事実は保持）／採用と同時に完了（AdoptedAsCompletion：到着 record 済み・現在リビジョンと一致・delegation（execution lifetime）から列挙した authoritative set が claim と完全一致・依拠試行が ConfirmedSuccess または検証済み無作用失敗への記録済み非依拠判断であり・同じ TaskId の全 revision / 全 delegation に Unknown が無い（Task-wide completion barrier））／元の依拠リビジョンへ記録のみ（RecordedToOriginalOnly：revision 前進・progress が terminal（Completed / Failed / Cancelled））／完了を保留（WithheldByEffectFacts：blockers = authoritative set に Unknown / 非依拠判断が成立しない失敗 ∪ 同じ TaskId の Task-wide barrier に残る Unknown。重複は 1 回。到着 record と result-local 相関は残り現在 Task は不変。seal 後の証拠更新で再評価可能）／採用対象の result 行が無い（MissingResult）／委任・タスクの durable 状態が無く書き込みなし（MissingDelegation / MissingTask。識別子不在は技術的エラーにしない）。同じ TaskId の別 delegation / 旧 revision の試行は barrier のためにのみ読まれ、`task_result_attempt` には刻印しない。同一 delegation への 2 つ目の final result 到着は AU15a の durable invariant 違反として fail closed（domain outcome ではない） |
| 経験提出・訂正・スコープ | `FormationDecision`、`CorrectionOutcome`、`ScopeDecision` | 知識形成（Formed）／保留（Deferred）／保存価値なし（Declined）／訂正完了（Corrected）／パートナー専用を維持（KeptAsCompanion）／明示制約により拒絶（DeniedByExplicitConstraint）／対象期限切れ（StaleTarget）／消去中保留（HeldForErasure） |
| 権限リアルタイム照合 | `LiveAuthorizationDecision` | 今回の利用を認可（AllowForThisUse）／拒絶（Deny）／オーナー確認待ち（AskOwner）／条件充足待ち（WaitForCondition）／再照合が必要（NeedsRevalidation） |
| 認証秘密利用 | `AuthenticatedUseOutcome` | 規定範囲で安全に利用（UsedWithinScope）／再認証が必要（NeedsReauthentication）／制約により拒絶（DeniedByConstraint）／参照期限切れ（StaleReference） |
| 推論実行・フォールバック | `InferenceDispatchOutcome`、`FallbackDecision` | 完了（Completed）／旧秘密の除去を証明できず結果本文を返さない（ResultUnavailable：利用量・外部作用の事実は保持）／送信前拒絶（NotSent）／ローカル協調停止（Aborted：claim 前の fast-path で abort を観測した場合は attempt も利用実績も残さず、claim 後は未送信と旧 ticket の送信不能を証明した場合だけ NotSent / 予約 Released とし、それ以外は不確定利用実績を記録してから Aborted を返す。claim 自体の拒否は通常の pre-send refusal であり Aborted へ丸めない。provider 要求・外部作用の停止は意味しない）／承認済みフォールバックとして許可（AllowedAsApprovedFallback）／未承認経路のため拒絶（DeniedAsUnapprovedRoute） |
| 利用枠予約・確定・解放 | `ReservationOutcome` | 予約成功（Reserved）／上限超過で拒絶（DeniedByCap）／費用不明のため保留（HeldForUnknownCost）／再照合が必要（NeedsRevalidation） |
| アクション開始・確定 | `ActionStartOutcome`、`LateArrivalHandling` | 試行開始（StartedAsAttempt）／拒絶（Denied）／オーナー指示待ち（AskOwner）／前提不一致（StalePremise）／消去中のデータ利用保留（DataUseHeld：covered / unknown。未開始なら試行・作用 0）／タスク終端（TaskTerminal：Completed / Failed / Cancelled のため開始なし。Action 側は Task lifecycle 語彙を import しない）／実行 seal 済み（ExecutionSealed：delegated execution が final result で seal 済みのため開始なし。Task lifecycle 語彙を import しない）／全体保留中（HeldByGlobalHold）／元記録へ保存（RecordedToOriginal）／成否不明のまま重複リスク提示（KeptUnknownWithDupRisk）／消去条件により再保存抑止（SuppressedByErasure） |
| 在席移動・対話ラウンド | `MoveDecision`、`RoundIntakeOutcome` | 新端末へ移行開始（TransitioningToNew）／古い在席情報のため拒絶（RejectedAsStalePresence）／制約により拒絶（DeniedByConstraint）／ラウンド受理（AcceptedForRound）／過去ラウンドのため拒絶（StaleRound）／移行中保留（HeldForTransition） |
| 観測結果ルーティング | `RoutingDecision` | 指定パートナーへ配信（RoutedTo）／制御設定により抑止（SuppressedByControl）／古い観測候補のため破棄（StaleCandidate） |
| キャラクター適用・インポート | `CharacterApplicationOutcome`、`SkillImportOutcome` | 適用完了（AppliedAs）／定義 revision または個体の適用 current の不一致（StaleApplicationPremise：現行状態付き、再選択が必要）／現在の消去条件により適用を保留（DataUseHeld：履歴追加なし）／禁止部品のため拒絶（DeniedProhibitedPart）／オーナー選択待ち（NeedsOwnerSelection）／スコープ別インポート完了／スコープ規則違反で拒絶（RejectedByScopeRule）／検証失敗で拒絶（RejectedByValidation） |
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
| タスク委任・自律エージェント | `CreateDelegationCommand`（期待リビジョン ＋ `task.progress` 非 terminal の不可分な比較照合。AU3 の作成スライスは委任 ID・依拠 TaskRef・委任元・ephemeral ID・スコープの写しを永続化し、progress を `Started → InProgress` へ進める。委任 = 1 delegated Task Agent execution の lifetime） | `TaskAgentResultArrival`（finalization 境界で到着 record を先に durable 化し、同じ不分区間で delegation を seal。その後に採用判定。安全な結果本文または body-free held の identity と 1 回だけの保存、claim の authoritative set 検証、Task-wide completion barrier の検証、採用と完了確定） | `(結果ID, 委任ID（execution lifetime）, 依拠 TaskRef, 安全な結果本文 1 行または body-free held, 依拠 Action 試行相関（authoritative set。seal 時点で固定。result-local）, 採用リビジョン, Task progress)`。完了は result-local 相関の検証と Task-wide unresolved Action completion barrier（同じ TaskId の全 revision / 全 delegation に Unknown が無いこと）の両方で確定する。依拠リビジョンは delegation 行、目的はそのリビジョンの `task_revision` snapshot から解決。execution lifetime は 1 delegation（0..N inference turn / 0..N Action attempt / 0..1 final result）であり、final result 到着で seal する。Action 試行の確定度は実行・拡張 owner の事実のまま参照のみ。再起動後は `(result_id → task_result → delegation → task_revision → task_result_attempt)` の bounded read（`load_task_result`）で辿り、到着済み・採用未確定の recorded 結果だけは本文を保持して提示でき、erasure-held と body-erased は本文なしで示す。採用済み body-erased は過去の採用 identity / Action 相関のみ示す。PR グループD（Task）/E（Action）、CI §5.3 |
| アクション試行・外部ツール・Computer Use | `ExecuteActionCommand`（実行前の不可分な比較照合。`task.progress` 非 terminal と execution 非 seal を含む）→ `StartedAsAttempt(attempt)` | `ReportEffectFact`（試行ごとの CAS 更新）＋ `LateArrivalAttribution`（遅延到着の帰属） | `(試行ID, 委任（execution lifetime）, タスクリビジョン前提, 実際の操作対象と操作種別, 依拠した認可ID, 在席／復元世代, 元の成否不明試行ID)`。PR グループE |
| バックアップ作成 | `CreateBackupCommand` | `BackupPointFact`（対象時点、参照関係、未完了状況の整合性が揃って確定） | `(バックアップID, 対象時点・参照整合性・除外データ・未完了状況)`。PR グループJ |
| バックアップ復元 | `RequestRestoreCommand` → `StagedRestoreCandidate`（隔離ステージング検証） | `SwitchRestoreDecision`（復元世代の更新 ＋ マスター切り替え）→ `BulkEnableAfterRestoreCommand` | `(復元ID, バックアップID, RestoreGeneration, 安全保留, 認証情報照合)`。PR グループJ |
| 個人データ完全削除 | `RequestTargetedDeletionCommand` → `DeletionScopeDecision` | `DemandLocalErasureCommand` → `ParticipantCompletionFact` → `VerifyRemainderQuery` → `GlobalDeletionCompletion` | `(操作ID, 消去スイープ世代, 有効期間, 参加者対応, 安全保留)`。PR グループJ、CI §5.9 |
| 未伝達メッセージ報告 | `RegisterUndeliveredFact`（親の処理と不可分に登録） | `RequestUndeliveredSummaryQuery` → `UndeliveredSummaryFact` → `PresentationMark`（要約本文全体の表示または音声再生完了の確認後にのみ確定） | `(未伝達ID, 元データとの対応, 報告状況, ラウンド・世代対応)`。PR グループB |
| キャラクター適用 | `GetCharacterRevisionQuery`（資材供給） | `ProposeCharacterApplicationCandidate`（オーナー選択を伴う提案 → 適用確定） | `(CompanionId, CharacterId, 期待する定義 revision, 期待する適用 current の版・内容, 選択部品, OwnerSelectionRef の操作 identity)`。PR グループA/B |

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
    origin: TaskContextOrigin,       // steering は OwnerConversation、resume は H-A.1 の Owner source
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
    TaskTerminal { task: TaskId, progress: TaskProgress }, // 対象 Task が terminal（Completed / Failed / Cancelled）。
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
// - 同じ不分区間で現在の task.progress が非 terminal であることを必須とします。terminal（Completed / Failed / Cancelled）は
//   TaskTerminal { task, progress } として何も書き込まずに返し、StaleExpected と区別します（吸収的な終端であり、
//   再評価で前進できる stale ではありません）。この gate は cancel の `cancelled` を含む terminal 集合にそのまま適用され、
//   cancel 専用の比較条件を追加しません。
// - MissingTask / RevisionExhausted / TaskTerminal は正常系（Ok 側）のドメイン判定結果（domain outcome）とし、永続化状態を変更しません。RevisionExhausted は後続リビジョンの不在だけでなく、後続リビジョンを永続化表現に写せない場合も含みます。
// - AU4 のリポジトリスライスが記録するコンテキスト種別は採用目的のみです。最初の追加種別である採用指示は、H-A steering 配線スライスが書き込み側の前提（adopted_instruction）・項目種別識別子（discriminator）・マイグレーション・読み取り規則の拡張を同一の設計変更として追加します。材料や途中理解はそれぞれの利用先が分岐するスライスが同一の形式で追加します。どの種別も revision 前進と同一 transaction に載せます。steering は forward_steering（AU4）、明示 resume は同じ前進 helper と委任作成を組み合わせた AU17 を使い、別 revision / 別 transaction の指示採用を作りません。
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
    kind: TaskCreationKind,
}
enum TaskCreationKind {
    Normal { workspace: Option<WorkspaceAssociationPremise> },
    BackupExport { source: BackupTaskSource }, // StartedOccurrence または Owner RunNow。Workspace / delegation は持たない
}
struct WorkspaceAssociationPremise {
    assoc: WorkspaceAssocId,
    need: WorkspaceNeedRef,
}

// V47 の BackupExport は task_kind / backup_task_source と Task・初期 revision / context を
// 同じ commit で確定する。Normal だけが Workspace を持ち、BackupExport は持たない。
// Started occurrence の origin は同じ ScheduleOccurrence / 発生 ID、Run now は指示の
// OwnerConversation / OwnerManagement / source ID と一致させ、assignee・目的も schedule と照合する。
// AU18 は別の create_task commit を呼ばず、同じ typed premise を内部同期 writer に渡して
// occurrence Started・Task 参照・区間 cursor・Task 一式を同じ Immediate で確定する。
// Run now は create_task の BackupExport で認証済み Owner source・現在 schedule・担当
// Running Companion・hold を同じ commit で比較する。同一 source retry は同じ TaskRef、
// 別 Task / schedule への source 再利用は拒否する。Task の後追い種別補完は禁止（PR AU2 / AU18）。

// 委任作成の全内容（AU3）。delegation identity と ephemeral identity は owner（作業）が
// 発行し、リポジトリは採番し直しません。delegator は premise では渡さず、リポジトリが
// 同一トランザクションで現在の Task 行の担当者から写します（委任の有効性は常にタスク側の
// 現在単位から再確認し、この写しだけを根拠にしません）。scope_copy は作業担当（orchestrate）
// が依拠した Task 単位の確定済みワークスペース関連付けから写して構成します。
// 委任作成は Task revision を前進させませんが、progress を Started → InProgress へ進めます。
// 同じ不分区間で現在の task.progress が非 terminal であることを必須とし、terminal（Completed / Failed / Cancelled）は
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
    progress: TaskProgress,          // 開始 / 進行中 / 完了 / 失敗 / 中断（revision とは別軸。中断は受理の durable marker そのもの）
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

// 1 つの結果の durable record（D2）。保存を許された本文だけが body の正本であり、複製しません。
// final result の到着 record が先に安全な本文または body-free held と identity を確定し、同じ不分区間で delegation を
// seal します（durable invariant: 1 delegation につき最大 1 行）。採用判定はその同じ行に対して行われます。
// `attempt_refs` は採用判定で authoritative set と完全一致が確認され、`task_result_attempt` に刻印された
// opaque な相関だけを返します（未採用・未判定なら空であり得る。Action owner の newtype ではなく opaque 相関）。
struct TaskResultRecord {
    result: TaskResultId,
    task: TaskRef,                   // 依拠リビジョン（delegation 行の task_revision と一致）
    delegation: DelegationId,        // execution lifetime（1 delegated Task Agent execution）。seal の単位
    body: Option<TaskAgentOutput>,   // Recorded のみ Some、他の disposition は None（Debug では伏字化）
    arrival_disposition: TaskResultArrivalDisposition,
    attempt_refs: Vec<RawId>,        // 採用判定で検証済みの authoritative set（task_result_attempt の相関）
    non_reliance: Vec<RecordedNonReliance>, // 採用済みの判断。消去後は理由なし、試行・根拠 identity は保持
    achievement_evidence: Vec<RawId>, // 非依拠採用時の目的達成を確認した客観的記録への参照
    remaining_work: Vec<String>,      // 採用時に検証した残作業（非依拠採用では空）
    adopted_revision: Option<TaskRevision>, // Some は現在 Task へ採用された（adopted result）。None は到着 record のみ
    recorded_at: WallClockWithTz,
}
enum TaskResultArrivalDisposition { Recorded, ErasureHeld, BodyErased, LegacyBodyUnverified }

// BodyErased は到着時の選択ではなく、保存済み Recorded 結果への削除参加による遷移です。
// adopted_revision が Some でも過去の採用事実として保持し、現在の result 本文は返しません。

// 採用判定の主張。caller（orchestrator / Task Agent 経路）が「この result が依拠した」と主張する
// Action 試行の列挙であり、それ自体は完了の根拠になりません。store は result 行の delegation
// （execution lifetime）から authoritative set を列挙し、この集合との完全一致（欠如・追加・重複の
// いずれも不可）を要求します。
struct TaskResultAdoptionClaim {
    result: TaskResultId,
    attempt_refs: Vec<RawId>,
}

// H-A の TaskCompletionAssessment / VerifiedNonReliance と同じ採用入力型。
// 読戻しは H-A の RecordedNonReliance / NonRelianceReasonDisposition を使用する。
// Task owner が現在の目的・残作業を検証し、客観的な達成根拠を対応付けて供給します。
// store は Action owner の grounds、試行対応、根拠参照と現在性を採用 commit で再照合します。
// 根拠を検証できないときは採用しません。新たな LLM 自己申告だけでは根拠になりません。
struct TaskCompletionAssessment {
    non_reliance: Vec<VerifiedNonReliance>,
    achievement_evidence: Vec<RawId>,
    remaining_work: Vec<String>,
}
// VerifiedNonReliance は H-A の定義を参照する。採用時には reason が必須。

trait TaskRepository {
    // タスクの新規作成：task + task_revision + 初期 task_context_entry + kind 別の
    // workspace_assoc または backup_task_source を不可分に永続化。AU18 の予定回は同じ
    // 同期 writer に occurrence / cursor も含める。コミット前は外部実行から不可視。
    // 挿入に失敗すれば全体をロールバックし、種別や由来が欠けた Task を公開しない。
    async fn create_task(
        &self,
        premise: TaskCreationPremise,
    ) -> Result<TaskRef, TaskTechnicalError>;

    // 方針指示（steering）：新リビジョンと新コンテキストを不可分に前進（過去リビジョンも確実に保持）。
    // 採用するコンテキスト項目の識別子は前提条件（premise）が保持し、リポジトリはCAS成立後のリビジョン参照のみを刻みます。
    // 同じ atomic compare で現在の task.progress が非 terminal であることを必須とし、terminal（Completed / Failed / Cancelled）は
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
    // 採用指示項目の本文はこの読み出しで History から引いてはなりません（Task の読み出しに History owner の
    // テーブル走査を混ぜない）。本文の解決は推論ターンの入力組み立てが、作業側の TaskInstructionSource port
    // （message_id PK の単一メッセージ bounded read）で行い、TaskRecord へ本文を複製しません。
    async fn load_task(
        &self,
        task: TaskId,
    ) -> Result<Option<TaskRecord>, TaskTechnicalError>;

    // タスク中断（Cancel、AU16）：1 つの短い Immediate トランザクションで現在の task.progress を読み、
    // started / in_progress のときだけ cancelled へ CAS します。この commit が「中断要求を受理した」唯一の
    // durable な事実であり、「実際の処理・外部作用が停止した」ことは意味しません。停止完了を待たず、
    // 停止完了を受理の条件にもしません。completed / failed は TaskTerminal、既に cancelled は
    // AlreadyCancelled として書き込みなしで返し、識別子不在は MissingTask とします（いずれも Ok 側の
    // ドメイン判定。未知の progress 値・競合による更新行数の不一致は技術的エラー）。
    // 会話 source の intent PK から Owner message / accepted sequence・Owner role・Companion・対象 Task・直接的中断意図を、
    // 後続の同じ intent / Task に結ぶ型付き撤回・確認待ちを索引付き限定範囲で CAS と同じ master から照合します。
    // History 本文全件の走査・モデル推測・最新 accepted Owner message 比較は行いません。
    // 無関係な後続入力は Superseded ではなく、曖昧なら NeedsClarification で確認まで保留します。
    // 管理 source は認証済み操作の source と対象 Task の同一性を照合し、会話の比較は課しません。
    // リビジョン・目的の前提は持ちません：中断はタスク単位の操作であり、同時 steering と競合しても
    // stale にせず、勝った順に durable に残します（steering が先なら revision 前進は取り消しません）。
    // 開始済みの inference attempt / data_use 相関 / action_attempt には遡及・削除・書換えを行いません。
    // cancel が禁じるのは新規 work の admission と現在 Task への採用・lifecycle 前進だけであり、
    // already-started activity の事実記録（AU15a の到着 record と seal、AU15b の検証済み
    // task_result_attempt 相関、Action certainty の CAS、利用量などの事実記録）は妨げません。
    // cancel 受理・停止状態の専用列・行・フラグ・gate 条件は追加しません。会話 source の
    // body-free intent / followup は帰属事実です。cancelled は terminal progress であり、
    // 既存の非 terminal gate（AU3/AU4/AU14/AU5/AU15b）がそのまま拒否します。
    // 復元・安全保留を跨ぐ要求の失効や hold 中の受理可否は、保全・消去スライスが hold 前提と対で
    // この同じ比較点に追加します（本スライスは generation premise を先取りせず、第 2 の cancel gate を作りません）。
    async fn cancel_task(
        &self,
        command: CancelTaskCommand,
    ) -> Result<TaskCancelOutcome, TaskTechnicalError>;

    // 委任の作成：expected_task_revision の比較照合と、現在の task.progress が非 terminal であることを
    // 満たした上で安全に作成し、progress を Started → InProgress へ進めます。delegation と
    // agent の識別子は作業担当（orchestrate）が発行して premise で渡し、リポジトリは採番し
    // 直しません。delegator は同一トランザクション内で現在の Task 行の担当者から写し、
    // 同じリビジョンの task_revision snapshot との担当者一致を確認します（欠如・不一致は
    // 技術的エラー。合成値を作りません）。リビジョン不一致（StaleTaskRevision）、タスク
    // 未存在（MissingTask）、terminal（Completed / Failed / Cancelled を TaskTerminal { task, progress }）は commit せずに返し、
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
    // 未確定の到着だけ、sealed ScrubbedText の証明と expected_credential_set の一致、現在の
    // credential-set revision、および delegation の durable inference attempts の旧 used-version
    // authoritative set と証明 set の一致を同じ Immediate トランザクションで照合する。現在値や
    // set の変化は StaleCredentialSet { current }（証明と引数の不整合は技術的エラー）として本文も seal も書かない。
    // 保存するのは結果 identity・許可された scrub 済み本文または body-free held・delegation 行から解決した依拠 TaskRef・
    // recorded_at であり、adopted_revision は NULL、attempt 相関は書きません（採用判定で検証後 1 回だけ刻印）。
    // durable invariant: 1 delegation につき final task_result は最大 1 行（UNIQUE(delegation_id) 相当）。
    // 同じ delegation に既に final result がある場合、同じ result identity なら元の到着事実・委任・依拠リビジョンが
    // 一致するときに限り現在の disposition を返し、異なる result identity は 2 つ目の final result として受理せず
    // fail closed（技術的エラー）します。事後 BodyErased は到着時 disposition の改ざんではなく削除の事実です。
    // 確定済み ID の retry は元の result・delegation・依拠 TaskRef の帰属を照合し、recorded で
    // 本文が残るなら到着本文も比較します。不一致は技術的エラー。本文なしの三状態では
    // 本文を比較・復元・再充填せず帰属だけを照合します。外部作用は再実行しません。
    // 一致しない identity 再利用は技術的エラーとして既存行を上書きしません。task revision・progress・確定度・terminal は判定しません
    // （採用・完了は adopt_result の役割）。1 回の inference turn の provider 出力（Produced）は final result では
    // ないため、この step へ入れて seal してはなりません（intermediate output の seal 禁止）。
    async fn record_task_result_arrival(
        &self,
        arrival: TaskAgentResultArrival,
    ) -> Result<TaskResultArrivalOutcome, TaskTechnicalError>;

    // 委任の final result の bounded read：delegation_id に対応する final task_result（0..1 行）を読み戻します。
    // 行の存在そのものがその delegation の execution seal（該当 execution の新しい inference claim / Action 開始を
    // 拒否する根拠）であり、結果 identity・本文・採用リビジョンから再起動後の対応を辿る入口です。存在しない場合は None。
    // 同じ result identity の再 arrival の冪等性確認と、作業側の再読込マッピング（seal 済み → ExecutionSealed）が使います。
    // V44 の ErasureHeld（body NULL、採用なし）と BodyErased（body NULL、採用済みの場合もあり）は
    // 正常な seal 済み行として返します。Recorded での本文欠如、ErasureHeld の採用、未知の disposition、
    // 採用リビジョンと依拠リビジョンの不整合は技術的エラーとし、合成した record を返しません。
    // 採用済み非依拠理由は RecordedNonReliance で読み、理由の Recorded/Erased と Some/None の
    // 対応を検証します。Erased は参照先の本文を再生成せず、試行・根拠参照だけを返します。
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
    // terminal transition を行わず同じ AdoptedAsCompletion を返します（BodyErased でも採用事実のみ返し本文なし）。
    // adopted_revision と依拠リビジョンの不整合は技術的エラーです。現在 revision が前進済み・task.progress が terminal（Completed / Failed /
    // Cancelled）の場合は、attempt 相関だけを 1 回だけ刻印し、現在 Task を変更せず RecordedToOriginalOnly を返します。
    // ErasureHeld は ErasureHeld、未採用 BodyErased は BodyErased を返し、採用・完了へ進めません。
    // 現在 revision と一致する Recorded の場合は、
    // 同じ不分区間で Task-wide completion barrier を検証します。barrier は、同じ TaskId に属する全 durable
    // ActionAttempt（全 revision / 全 delegation。steering 前の古い revision の started Action を含む）のうち
    // certainty が Unknown の集合が空であることを要求し、1 件でもあれば Completed への CAS を行いません。
    // barrier の入力は action_attempt の durable facts（task_id / task_revision / delegation_id / certainty）
    // だけで、timestamps・delegation liveness・ephemeral alive flag・result body・自己申告・caller の
    // attempt_refs は使いません（不整合な行は技術的エラーとして fail closed）。result-local authoritative set は
    // 「result が何に依拠したか」の相関であり、barrier は「完了後に未確定の外部作用が確定しないか」の gate で、
    // task_result_attempt に刻印するのは result-local の検証済み集合だけです。cross-delegation / 旧 revision の
    // ConfirmedSuccess / ConfirmedFailure は barrier だけを理由に完了を block しません。inference attempt と
    // unsealed delegation の存在は barrier に含めません。Task owner は現在の目的に対する代替達成、残作業、
    // 客観的根拠を確認し、確認済み無作用の失敗に限って「完了判定で依拠しない」と判断できます。
    // 部分的な作用、根拠不足、未完了作業は保留し、Unknown は非依拠にできません。判断と根拠は
    // task_result_attempt に確定度とは別に採用 commit と不可分に刻印します（Action owner の certainty は不変）。
    // blockers = 非依拠が検証できない result-local の失敗/Unknown ∪ Task-wide の Unknown
    // （集合。重複は 1 回）が空でない場合は、attempt 相関（result-local のみ）を刻印した上で
    // WithheldByEffectFacts { attempts: blockers } を返し、Task を完了しません（確定度は実行・拡張 owner の
    // 事実のまま読み取るだけで、Task owner は書き換えません）。
    // membership は result 到着（seal）時点で固定されており、seal 後に ene-action owner の客観的証拠が
    // Unknown を ConfirmedSuccess / ConfirmedFailure へ進めた場合は、この同じ result の再評価で完了が
    // 成立し得ます（membership と certainty を混同せず、membership を増減させません）。
    // authoritative set が durable に空の場合のみ Action 無しとして採用判定へ進みます（claim が空だから
    // Action が無かった、と推定しません）。blockers が空で、Task owner による目的達成の検証が成立する
    // 場合は、同じトランザクションで task_result.adopted_revision を現在 revision に刻み、task.progress を
    // 非 terminal から Completed へ CAS して AdoptedAsCompletion を返します。
    // 同じ result identity の再 arrival は消去済み本文を復元せず、記録済みの adopted_revision と現在状態から
    // 同じ判定を再評価して二重採用・二重完了を防ぎます。delegation 行または task 行が
    // 存在しない場合は MissingDelegation / MissingTask として書き込みなしで返します（識別子不在は
    // 正常系のドメイン判定、部分的・不整合な行は技術的エラー）。
    async fn adopt_result(
        &self,
        claim: TaskResultAdoptionClaim,
        assessment: TaskCompletionAssessment,
    ) -> Result<TaskResultAcceptance, TaskTechnicalError>;

    // 結果の bounded read：result identity から、依拠 TaskRef・delegation（execution lifetime）・許可された本文・
    // disposition・検証済み attempt 相関・採用リビジョン・非依拠理由の現在 disposition を読み戻します。存在しない場合は None。
    // ErasureHeld / BodyErased / LegacyBodyUnverified の body None は合法であり、採用済み identity と相関も返します。
    // Recorded の本文欠如、ErasureHeld の adopted_revision Some、未知の disposition や採用リビジョンと
    // task の不整合、Recorded 理由の欠如 / Erased 理由の残存は技術的エラーとし、合成した record を返しません。
    // Erased の理由は復元せず、attempt / evidence_refs は body-free な identity として返します。
    // 再起動後に結果 ⇔ 委任 / 依拠リビジョン / Action 試行の対応を辿るための入口です。到着 record 済み・
    // 採用未確定（adopted_revision = None）の Recorded 結果は、本文を保持したまま「到着済み・採用未確定」として
    // 読み戻せます。body-free 行は現在の結果本文として表示・報告せず、到着時保留か事後消去かを区別します。
    // 行の存在だけでは replay・採用を許可しません。Recorded のみ PR §6.4 の全 gate を満たす AU15b 再評価を許可します。
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
// このスライスの list / read / create / edit は、既存の依拠タスクリビジョンと現在の
// workspace_assoc を前提とします。Workspace 消去候補は current-condition 判定を持つ
// スライスで対として有効にします。依拠権限評価・在席・費用予約・復元の前提は、
// それぞれの producer を持つスライスがフィールドと比較を同じ設計変更で追加し、
// それまで代役値や placeholder 列を置きません（AU2/AU14 と同じ規則）。
// 他ドメインの newtype を直接 import せず、素の ID ＋ 世代数値で受け取ります（CM §4.3）。
struct WorkspaceErasureAssessmentRef(/* 保全・消去 owner が発行する非永続の候補・条件比較参照 */);
struct WorkspaceErasureCandidate<'a> {
    known_sources: &'a [RawId],      // 既知の canonical source lineage。欠落・判定不能は許可しない
    write_body: Option<&'a [u8]>,    // Create / Edit は確定した実書込本文全体、List / Read は None
    assessed_current: WorkspaceErasureAssessmentRef, // 非永続。active condition set の generation / phase と完了後 provenance cut を束縛
}

enum ActionErasureCandidate<'a> {
    TaskWorkspace(WorkspaceErasureCandidate<'a>),
    BackupTaskExport {
        known_sources: &'a [RawId],
        effect_body: &'a [u8],
        assessed_current: WorkspaceErasureAssessmentRef,
    },
    Direct {
        known_sources: &'a [RawId],
        effect_body: Option<&'a [u8]>,
        assessed_current: WorkspaceErasureAssessmentRef, // 保全・消去 owner の同じ current condition / cut。Workspace 関連付けは不要
    },
}

enum AttemptAssociation {
    TaskBound { task: RawId, task_revision: RevisionInner, scope: AttemptTaskScope },
    Direct { instruction_source_kind: OwnerInstructionSourceKind, instruction_source: RawId, companion: Option<RawId>, target_scope: RawId, system_operation: Option<SystemActionOperation>, manual_backup_point: Option<RawId> },
}
enum AttemptTaskScope {
    Workspace { delegation: RawId, workspace: RawId },
    BackupTaskExport { source: AttemptBackupTaskSource, backup_point: RawId, output_scope: RawId, setting_revision: BackupSettingRevision },
}
enum AttemptBackupTaskSource {
    StartedOccurrence { schedule: RawId, occurrence: RawId },
    RunNow { schedule: RawId, instruction_source_kind: OwnerInstructionSourceKind, instruction_source: RawId },
}

enum OwnerInstructionSourceKind { Conversation, Management }

struct AttemptCommitPremise<'a> {
    attempt: ActionAttemptId,        // 識別子はオーケストレーションが発行し、リポジトリは採番しない
    association: AttemptAssociation, // TaskBound は TaskRef と閉じた用途別 scope、Direct は Owner source / optional Companion / 対象 scope / optional system operation
    real_target: RealTargetRef,      // 実行直前に解決された実際の対象
    operation: OperationKind,        // List | Read | Create | Edit（このスライスの closed world）
    relied_evaluation: RawId, // K-B.1 の single-use evaluation の opaque な対応識別子。Permission 所有の semantic newtype そのものではなく、mapping は authorization / orchestration 境界で行う
    erasure_candidate: ActionErasureCandidate<'a>, // 帰属と variant を一致させる。本文は借用のみで永続化しない
}

enum ActionStartOutcome {
    Started,                         // 試行行は durable。実行はトランザクション外
    ExistingManualBackupAttempt { attempt: ActionAttemptId, certainty: ActionCertainty }, // source に既存の試行。挿入・作用なし
    DataUseHeld,                     // 消去 coverage / 完了後 cut / 判定不能。試行行・作用なし
    StalePremise,                    // 委任・タスク・ワークスペース前提の欠如/前進。書き込みゼロ
    TaskTerminal,                    // 対象 Task が terminal（Completed / Failed / Cancelled）のため開始しない。書き込みゼロ。
                                     // Action 側は Task lifecycle の値型を import せず、Task owner が自分の
                                     // TaskTerminal { task, progress } として意味を説明する
    ExecutionSealed,                 // delegated execution が final result で seal 済みのため開始しない。書き込みゼロ。
                                     // Task lifecycle の terminal とは別（Task は InProgress のままであり得る）。
                                      // Action 側は Task lifecycle の値型を import しない
    CompanionUnavailable,            // Stopped / Deleted / deletion hold。書き込みゼロ
}

// 確定度の CAS 結果。Updated 以外は書き込みなしのドメイン結果。
enum CertaintyUpdateOutcome {
    Updated,
    StaleCurrent { current: ActionCertainty },
    MissingAttempt,
}

trait ActionAttemptRepository {
    // Workspace TaskBound は委任・依拠 Task revision・現在の workspace_assoc / scope_assoc・非 terminal
    // progress・未 seal を AU5 で照合する。Backup 専用出力は Task 種別 / source / point / scope / 設定・委任不存在を照合する。
    // Direct は Owner 指示 source・対象 scope と該当時の Companion / system operation を比較し、Task / 委任を要求しない。
    // Companion 付き帰属は Running / deletion hold を Stop と同じ DB 比較点で照合する。None は管理操作・列挙した用途に限る。
    // relied_evaluation は single-use とし、同じ評価 ID での
    // 二度目の挿入は技術的エラーとして拒否する（二重開始を許さない）。
    // 帰属・消去候補の variant の不一致、委任行と premise の不一致、複数行の関連付け、未知の操作種別は技術的エラーとし、
    // 推測で stale に丸めない。委任行（execution lifetime）は試行行へ保存され、結果採用の
    // authoritative set 列挙（delegation → action_attempt）の正本になる。seal 前に durable start した
    // 試行は seal の有無にかかわらず試行行に残り、authoritative set から消えない。
    // erasure_candidate は保全・消去 owner が transaction 外で帰属ごとに機械的に評価した同一本文・source・操作・
    // real_target の候補であることを照合し、同じ Immediate の中で canonical current condition の
    // generation / phase と完了後 provenance cut の有効性を比較する。covered / 判定不能なら
    // DataUseHeld（挿入なし）。世代・phase または候補が動けば外で再スキャンするか fail closed。
    // Read / List の対象・既知 source もこの比較で判定する。本文・hash・検索 material は永続化しない。
    async fn insert_attempt_if_current(
        &self,
        premise: AttemptCommitPremise<'_>,
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
    // 定義 revision・現在適用版・選択操作 identity を同じ短い DB 比較点で照合する。
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
    // list_unpresented は read-only、cursor/limit を owning query に適用する（PR §4.6）。
    // register は standalone async API にせず、親 fact の transaction 内の同期登録として提供。
    // 相手 owner は RegisterUndeliveredFact を渡し、source-key の一意制約で既存 ID を読み戻す。
    // begin_presentation は current receipt の選択行を送信前に PresentationUnknown にする。

    // 画面・音声の実提示が確認されて初めて Presented とする（durable-after-confirmed。送信成功だけで提示完了にしない）。
    async fn compare_and_mark_reported(
        &self,
        id: UndeliveredId,
        expected: ReportStatus,
        mark: PresentationMark,      // X-H で検証済み receipt の提示観測（Round / id 集合を保持）
    ) -> Result<ReportStatusTransition, UndeliveredTechnicalError>;
}
```

`WorkspaceErasureAssessmentRef` は保全・消去 owner の機械的判定を同じ候補に結び付ける非永続の比較材料であり、`ErasureConditionRef` の snapshot や「active condition が空」という自己申告を許可証にしません。`Create / Edit` では `write_body: Some` に保持した確定済み実書込本文をそのまま作用に使い、`Read / List` では対象・既知 source を AU5 で照合してから取得本文・列挙名の最終判定へ進みます。試行行には body-free な source lineage・対象 identity・削除開始との順序を残し、検索 material と本文・本文 hash は残しません。AU5 の成功後も作用直前の同一本文・source・対象の currentness は deletion start と直列化した短い同期区間で再照合し、開始が区間外へずれたら再照合します。結果本文の受入・返却時も現在条件と provenance cut を owner が確認し、旧由来・covered / unknown は本文なしの事実だけを保持します。既存試行の `Unknown` は無作用の証拠なしに変更しません。

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
3. オーナーからの追加指示は、`ProposeSteeringCommand(premise=SteeringPremiseRef, new_purpose, instruction_source)`（`unadopted` フィールドは W-3 の反映判断を生成元に持つスライスで導入）によって、新リビジョンと新コンテキストの不可分な前進（forward）となります。新リビジョンは採用目的項目と採用指示項目を同一の `forward_steering` トランザクションに記録します。採用識別子は作業担当が確定し、採用指示項目自身の `TaskContextEntryId` が表します（`instruction_source` は由来レコードの参照であり、採用識別子そのものではありません）。過去のリビジョンも確実に保持されます。採用指示本文は History を正本として Task Agent turn の入力組み立て時に bounded read で解決し、複製しません（V-11）。2つの方針指示が競合した場合は同期区分 SD-Task の順序で直列化され、先に確定した方を優先して現在の状態とし、後から到着した要求は新しい現在の状態に対する再指示として評価します。
4. 方針指示（steering）が行われた後に、古いリビジョンを前提とした委任作成やアクション実行要求が届いた場合は、`StalePremise { current }` または `StaleTaskRevision { current }` として不受理にし、再評価へ差し戻します。Task が terminal の場合は revision stale と区別し、委任は `TaskTerminal { task, progress }`、steering は `TaskProposalOutcome::TaskTerminal`、Action は `ActionStartOutcome::TaskTerminal` として書き込み・開始ともに行いません。実行中だった古い委任はベストエフォートで停止・縮小させ、古い結果を勝手に新しい目的に採用してはなりません。Task Agent の推論開始も同じ照合に従い、依拠リビジョン・現在のタスクリビジョン・`task.progress` が非 terminal であること・delegation が seal 済みでないことの比較を推論試行の確定と同一の不分区間で行います（照合に失敗した新規推論は `TaskPremiseStale` として送信前に拒絶し、作業側の再読込が terminal を検出した場合は `TaskTerminal`、delegation の final result（seal）を検出した場合は `ExecutionSealed` へ写します）。
5. エージェントの処理結果は、Task Agent execution が final result を提出した明示的な finalization 境界で `TaskAgentResultArrival(delegation, result, body: ScrubbedText, expected_credential_set: CredentialSetRevision)` として、orchestrate が `TaskResultId` を発行し final result の可視化より前に `record_task_result_arrival` で durable に記録します（消去 held の本文は記録しない。同じ不分区間で delegation を seal し、1 delegation につき final result は最大 1 つ）。`TaskAgentTurnOutcome::Produced` は 1 回の inference turn の provider 出力であり、Action 要求や途中経過を含むため final result でも seal でもなく、到着 record に入れません。その後の採用判定は `TaskResultAdoptionClaim(result, attempt_refs)` と Task owner の `TaskCompletionAssessment` を `adopt_result` が 1 つの短い `Immediate` トランザクションで判定します。依拠リビジョンは delegation 行から解決し、その revision snapshot の目的 identity と現在の目的 identity を比較します（本文の文字列一致は使いません）。同時に result 行の delegation（execution lifetime）から Action attempt の authoritative set を列挙し、claim の `attempt_refs` との完全一致を要求します（欠如・追加・重複は技術的エラー。seal 後に membership は変化しません）。現在リビジョンが前進済みまたは Task が terminal の場合は `RecordedToOriginalOnly` として元の依拠リビジョンにのみ記録し、現在のタスクには不採用とします。現在リビジョンと一致しても、result-local に `Unknown` / 非依拠判断が成立しない失敗があれば、または同じ TaskId の全 revision / 全 delegation の Action 試行に `Unknown` が残れば（Task-wide completion barrier。blockers は両者の和集合とし、重複は 1 回）、`WithheldByEffectFacts` として完了しません（barrier で見つけた試行は result-local 相関として `task_result_attempt` に刻印せず、terminal CAS の可否だけを判定します）。古い承認情報を使ってタスクの中断を勝手に解除してはなりません。採用・競合・不明の詳細は V-10 で確認します。
6. **失われてはならない情報**: 単なる発言記録とタスク反映内容および未反映・保留指示の対応関係、方針指示の前後の目的の区別、委任範囲、ワークスペース境界、クライアント依存条件、タスクリビジョンの前提情報。

### V-3 Action candidate → authorization → external effect → timeout → late result（K-H・K-B・K-K・H-A）

1. 機能利用元が `ActionCandidate(principal_chain, task, delegation, workspace, purpose, described_target, operation, data_use, cost_risk, relied_intent_rule)` を組み立てます。候補構造体を組み立てられたこと自体は、実行許可を意味しません。
2. 実行・拡張担当は現在の関連付けのフォルダから対象を実行直前に解決し（canonical 化・フォルダ配下の照合・mount/reparse 境界の照合。`List` は directory、`Read`/`Edit` は regular file）、記録用 `RealTargetRef` と、同じ実対象に作用するための非永続な guard を得ます。続いて guard を保持したまま `ActionUseCandidate(operation, resolved_target, delegation, task, task_revision, workspace)` を K-B.1 の `authorize_action_use` に渡します。`AllowForThisUse` の評価 ID だけが開始を許し、`Deny` / `NeedsRevalidation` は書き込みも実行も行いません。推論用の `PermissionEvaluationId` / `EvaluationTracker` は流用しません。
3. `AllowForThisUse` の評価 ID を consume してその opaque な `RawId` に落とします。Action owner は `Create / Edit` で実際に書く本文を確定して非永続で保持し、既知 source lineage・保持した実対象とともに保全・消去 owner の機械的 coverage / provenance cut を transaction 外で評価します。`Read / List` は対象・既知 source を評価します。`AttemptCommitPremise(attempt, delegation, task, task_revision, workspace, real_target, operation, relied_evaluation, erasure_candidate)` を `insert_attempt_if_current` に渡し、同一の短い `Immediate` トランザクションで委任対応・依拠タスクリビジョン・現在のタスクリビジョン・現在の `workspace_assoc`・委任の `scope_assoc` と current condition generation / phase・cut の有効性を照合します。現在の `task.progress` は非 terminal、delegation は非 seal を必須とし（AU5）、同じ評価 ID の二度目の挿入は技術的エラーです。active set または候補が動けば外で再スキャンするか `DataUseHeld` で fail closed。covered / 判定不能も独立した `DataUseHeld`（試行 0・作用 0）です。`Started` は durable fact であって作用許可の持続ではなく、作用直前に同じ本文・source・保持対象の currentness を削除開始と直列化して再照合し、path を開き直して別実体に切り替えません。欠如・前進は `StalePremise`、terminal は `TaskTerminal`、seal 済みは `ExecutionSealed` として書き込みも実行も行いません。
4. 作用の確定度は実行・拡張担当が実行そのものの観測から記録します。`ConfirmedSuccess` は対象の事後確認（例: 書き込み後の読み戻し一致）を根拠とし、エージェント自身の「成功しました」という自己申告を証拠にしません。作用が起き得なかったことが確認できた場合は `ConfirmedFailure`、確認できない場合は `Unknown` のまま保持し、中断要求の受理、通信の成功、画面への表示、DB保存の成功、再接続、復元、端末移動などを理由にして勝手に成功や失敗へ書き換えません。試行行は開始時に `Unknown` で挿入され、確定度は行ごとの CAS でのみ更新します。
5. 遅れて届いた成功の証拠は、元の試行記録に対して `compare_and_set_certainty(attempt, expected=Unknown, new=Confirmed, grounds)` のアトミック更新（CAS）で記録します。新たな客観的証拠の確認を必須とし、エージェント自身の自己申告を証拠にしてはなりません。現在のタスクへ結果を採用するかどうかは、H-A の受付インターフェースにおいて改めて判定します（このスライスは採用を行いません）。成否不明となったアクションを再実行するには、新しい試行IDの発行と、重複リスクを明示したオーナー自身の再判断が必要です。
6. **失われてはならない情報**: 判定時の想定対象と解決済み実対象の対応関係、委任の不変性、確定度（確認済み成功／確認済み失敗／成否不明）、試行と外部作用の区別、遅延到着の帰属、重複実行防止に必要な永続記録。
7. **deletion → AU5**: `Create / Edit` の本文を確定した後、AU5 より前にその本文・既知 source・実対象を覆う消去条件が durable になれば、AU5 は同じ master の `Immediate` 比較で現在の coverage と完了後 cut を評価して `DataUseHeld` を返します。source が空でも本文の機械的 coverage は省略せず、判定不能も同じ hold。試行 0・ファイル作用 0 であり、`Denied` / `StalePremise` / technical error に丸めません。事前評価後に active condition の generation / phase が変わっていても、旧評価を再利用せず transaction 外で再スキャンするか fail closed です。
8. **AU5 → deletion → effect**: AU5 が先に commit した A1 は既知 source と body-free な identity / 順序を持つ already-started fact として削除参加に列挙します。作用開始直前には AU5 と同一本文・guard の実対象を削除開始と直列化して再照合し、deletion が先なら作用を止めます。比較後の区間外で新たに作用を始めず、同一性を証明できなければ別名の対象へ切り替えません。A1 の `Unknown` は停止依頼だけで `ConfirmedFailure` にせず、無作用を客観的に証明できる場合だけ CAS します。作用が先なら結果の再保存は current condition / provenance cut に従い、旧由来や covered 本文を戻さず、試行事実と確定度は保持します。
9. **Read / List の取得後**: 対象と既知 source が AU5 を通っても、それだけで file content や direct child の名前は返せません。取得した本文・列挙名を current mechanical condition と完了後 cut で返却・利用・再保存前に再照合し、covered / 判定不能なら本文・名前を返さず `DataUseHeld`。既存 attempt と外部読み取りの可能性は事実として保持し、作用が無かったと決めつけません。

### V-4 Provider request near cap → reservation → usage（K-D・K-G・K-E・K-B）

1. 利用元が用途と論理コンテキストを定め、推論担当が `ResolveAssignmentQuery(consumer, capability, current_consent)` を用いてプロバイダ経路を解決します。解決された経路は単なる派生データであり、設定変更や同意の更新があった後も過去の選択を勝手に有効とみなす根拠にはなりません。
2. 各利用担当者は `ReserveUsageCommand(consumer, attribution, upper_bound)` を発行し、権限・制約担当が実課金先に対する全適用capと復元前cap由来の短命な安全条件を解決して事前予約します。同一のトランザクション内で適用scope × UTC windowの上限・暫定境界と関連する利用実績（予約中 ＋ 確定済み ＋ 成否不明の合計値）、復元由来のgapと費用holdを照合します（SD-Cap）。上限超過または適用cap windowの費用不明なら処理を開始しません。capが未設定、または非適用windowにだけgapがある場合はそれだけで送信を止めません。
3. 予約完了後の推論呼び出しや外部ツール実行は、データベースのロックを持たずに並行して実行されます（他の要求をブロックしません）。実行中に認証失効、安全停止、保留が発生した場合は、ベストエフォートで処理を停止します。
4. 処理完了時は `CommitUsageCommand(reservation, actual=確定値／不明の区別)` によって不可分に更新し、未使用の予約枠を解放します。遅延して届いた利用量報告は、元の予約ID、試行ID、タスク、割り当て設定へ正しく帰属させます。未報告の利用量、処理中の利用、成否不明の消費を、勝手にゼロやリセットにしてはなりません（プロセスの終了、端末移動、キャッシュ消去、ログ整理の際にもリセットしません）。
5. 共有観測による画面キャプチャの検知処理は、対象クライアントおよび観測専用割り当てにおける単一の利用として計上し、端末にいるパートナーの人数分を二重計上しません。配信後に各パートナーが個別に行う推論処理は、独立した新たな実利用として同一の全体利用枠へ含めます。
6. **失われてはならない情報**: 報告された実測値、成否不明、処理中費用の厳格な区別、消費主体・用途・実課金先の対応関係、同一残額の二重消費の防止、適用capの上限超過や該当windowの費用不明時におけるデータを保持したままでの安全停止・判断待ち状態。復元前cap由来の安全条件を解除しても予約の`Unknown`は解消しません。

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
2. 各受入・保存先は、データの保存や採用をコミットする直前に、処理中に発行された消去条件（`ErasureConditionRef(operation, sweep, valid_interval)`）をロックを持たずに高速に照合します。到着・生成された情報について、「元データとの関係および生成日時」と「消去操作の有効期間」を突き合わせて判定します。削除処理区間内に遅延到着したり再生成されたデータはすべて消去対象とし、新しい経験データとして特別扱いして保持してはなりません。
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
2. 個体調整担当は、オーナーが差分を確認した時点の `AppliedCurrentState`（未適用の場合も含む）と部品ごとの明示選択を添えて、`ProposeCharacterApplicationCandidate(companion, character, expected_character_revision, expected_applied_current, selected_parts, owner_selection)` を提案します。認証情報、権限昇格、同意の代用、特権の付与、プラグインやMCPの権限、全体共有スキル、システム制御設定、ルール、費用上限などの適用禁止種別が含まれている場合は、更新対象から確実に除外します。
3. `compare_and_set_applied` は短い DB 比較点で定義 revision・現在適用版と内容・明示選択の操作 identity を照合し、一致した場合だけ適用関係と履歴を確定します（`AppliedAs`）。2 画面が同じ適用版から異なる部品を選択した場合、先に確定した側だけが適用され、後者は現行適用状態付きの `StaleApplicationPremise` となり、再表示・再選択まで書き込みません。パートナーの経験・記憶は初期化せず、キャラクター資材（Character担当）と適用関係（個体調整担当）のマスターの所有権を混同しません。Targeted Deletion の condition / phase 更新とも同じ比較点で直列化し、被覆あり・不明なら `DataUseHeld` で適用せず、既存個体が参照する改訂と管理下アセットは削除 participant の処理まで維持します。
4. パッケージ検証の成功を、機能の実行許可やセキュリティ認可と混同してはなりません（`PackageValidationReport` は受け入れ可否、含まれるコンテンツの範囲、信頼／非信頼の扱いを示すものであり、制御設定の変更ではありません）。
5. **失われてはならない情報**: 静的なキャラクターリビジョンと個体への適用関係の明確な分離、部品ごとのオーナーによる明示選択、パートナーの経験状態が初期化されないこと、推奨設定が存在することと実際の実行許可・有効化が別であることの認識、エクスポートデータへのプライベートな情報の混入防止。

### V-10 Task Agent result adoption と Task progress（H-A・K-H・K-K・SD-Task）

1. **正常系（current R1 → finalization → 到着 record/seal → 採用 → 完了）**: current Task が revision R1。委任 D1 は R1 を依拠して作成され（AU3 の同一トランザクションで progress は `Started → InProgress`）、D1 の execution lifetime で Action 試行 A1（Read）と A2（Create）が durable に記録されています。Task Agent の tool loop が複数 turn を実行し、final answer を提出した finalization 境界で orchestrate が `TaskResultId` X を発行し、`record_task_result_arrival(TaskAgentResultArrival { delegation: D1, result: X, body: scrubbed_body, expected_credential_set: scrubbed_revision })` で本文を durable 化して同じ不分区間で D1 を seal します。その後 `TaskResultAdoptionClaim { result: X, attempt_refs: [A1, A2] }` を `adopt_result` へ渡します。1 つの `Immediate` トランザクションで、result 行の存在、依拠リビジョン R1 と現在 `task.revision`、目的 identity、`task.progress` が非 terminal であることを確認し、delegation D1 から `action_attempt` を列挙して authoritative set {A1, A2} を解決し、claim との完全一致、各行の `(task, revision, delegation)` 対応、いずれも `ConfirmedSuccess` であること、および同じ `TaskId` の全 revision / 全 delegation に `Unknown` が無いこと（Task-wide completion barrier）を確認します。同じトランザクションで `task_result_attempt` の相関行を 1 回だけ挿入し、`adopted_revision = R1` を刻み、`task.progress` を非 terminal から `Completed` へ CAS して `AdoptedAsCompletion(R1)` を返します。再起動後は `load_task` が `progress = Completed` と `adopted_result = X` を返し、`load_task_result(X)` が X → D1 → R1 → A1/A2 の対応を bounded read で辿れます。
2. **steering race（R1 結果の遅延到着）**: D1 が R1 を依拠して実行中に、owner の steering が R2 を確定します。その後に R1 の結果が到着した場合、`adopt_result` は delegation 行が依拠した revision と現在 revision の不一致を同一トランザクションで検出し、claim と authoritative set の一致を確認した上で attempt 相関だけを記録して `RecordedToOriginalOnly` を返します。R2 は完了・上書きされず、progress も変化しません（目的 identity が偶然一致していても、revision correspondence が崩れていれば現在へ採用しません）。
3. **Action Unknown（未確定の外部作用）**: 結果本文が成功を主張していても、A2（Create/Edit）の確定度が `Unknown` のままである場合、`adopt_result` は authoritative set に A2 を含めて検証し、`WithheldByEffectFacts { attempts: [A2] }` を返し、`task.progress` は `InProgress` のままです。Task が「成功して完了した」と確定されることはなく、Agent の自己申告によって `Unknown` が `ConfirmedSuccess` へ昇格することもありません。`ConfirmedFailure` が `RefusedBeforeEffect`（無作用確認）なら、Task owner が現在目的の代替達成・残作業なし・客観的根拠を検証し非依拠判断を durable に記録した場合に限って完了可能です。部分的作用や根拠不足の失敗は保留します。同じ `TaskId` の別 delegation / 旧 revision に `Unknown` が残る場合も Task-wide completion barrier が完了を保留します（walkthrough 18）。cross-delegation の `ConfirmedFailure` だけを理由に barrier が block することはありません（walkthrough 23）。
4. **同一 result の二重 arrival**: 同じ `TaskResultId` X が再び届いても、`task_result` の PK により本文行は増えず（erasure-held / body-erased / legacy-body-unverified は本文なしのまま）、`task_result_attempt` も重複挿入されません。既存行の `adopted_revision`・disposition と現在状態から同じ判定を再評価するため、採用済みなら `AdoptedAsCompletion`（同じ TaskRef）を返して二度目の terminal transition を行わず、記録のみ／保留だった場合は現在の disposition に従い `RecordedToOriginalOnly` / `WithheldByEffectFacts` / `ErasureHeld` / `BodyErased` / `LegacyBodyUnverified` を返します。採用済み body-erased の `AdoptedAsCompletion` は過去事実のみを示し、本文は返しません。同一 identity の再 arrival で委任・依拠リビジョンが異なる場合、または本文が残る recorded で到着本文が異なる場合は技術的エラー（事後 `body_erased` と移行由来の `legacy_body_unverified` は本文を再比較しない）とし、既存行を上書きしません。同じ delegation に別の result identity Y の final arrival を試みる場合は AU15a が 2 つ目の final result として受理せず fail closed（walkthrough 16）。別の delegation D2 の result Y が完了後に到着した場合は本文 gate を通った場合に限り `RecordedToOriginalOnly` で履歴に帰属し、旧由来なら `ErasureHeld` です。
5. **terminal gate（Completed / Failed / Cancelled 後の新規処理拒否）**: Task R1 が `Completed`（`Failed` / `Cancelled` も同様）になった後、同じ current R1 のまま (a) `create_delegation` 要求 → AU3 が progress terminal を検出して `TaskTerminal { task, progress }` を返し、delegation は作成されない、(b) steering 要求 → AU4 が `TaskTerminal` を返し revision は前進しない、(c) Task Agent 推論開始 → AU14 の claim が terminal を検出し `TaskPremiseStale`（作業側で `TaskTerminal` へ写像）として provider I/O なし、(d) 残っている古い delegation D1 からの filesystem Action 開始 → AU5 が `TaskTerminal` を返し、`action_attempt` も外部作用も発生しない。terminal は吸収的であり、cancel（progress `Cancelled`）も同じ terminal 集合の 1 値として、既存の比較点で追加条件なしに拒否されます。terminal gate と execution seal gate は別概念であり、Task InProgress + delegation sealed の場合は後述の `ExecutionSealed` として拒否されます。
6. **omitted Unknown attempt（claim の部分集合）**: execution D1 の Action 試行として A1（`ConfirmedSuccess`）と A2（`Unknown`）が durable に記録されていたのに、caller が `attempt_refs: [A1]` だけを主張した場合、`adopt_result` は delegation D1 から authoritative set {A1, A2} を列挙し、claim との不一致（A2 の欠如）を技術的エラーとして fail closed します。Task は `InProgress` のままで、A2 を省略した完了は成立しません。
7. **empty attempt spoof（空集合の主張）**: execution D1 が実際に A1 を開始していたのに、caller が `attempt_refs: []` を主張した場合、authoritative set {A1} と一致しないため技術的エラーとして fail closed し、完了しません。逆に authoritative set が durable に空である場合（その execution が Action を 1 つも開始していない場合）だけ、claim: `[]` が一致して採用判定へ進めます。
8. **true no-Action result（Action 無しの採用）**: execution D1 が Action を 1 つも開始せず pure inference の final answer を提出した場合、delegation D1 からの列挙は durable に空集合になります。現在 Task/purpose が有効で terminal でなく、claim: `[]` が一致する場合、Action 無しの結果として `AdoptedAsCompletion` へ進めます（claim が空だから Action が無かった、と推定するのではなく、delegation D1 からの列挙が空であることを durable に確認します）。
9. **crash after finalization（到着 record/seal 後・採用前のクラッシュ）**: `record_task_result_arrival` が X の本文・delegation を durable 化して D1 を seal した後、AU15b 前に Host が crash した場合、再起動後も X は到着済み・未採用として読み戻せます（削除参加で事後に `BodyErased` なら本文なし）。PR §6.4 の起動処理は `Recorded` に限って既存 facts に対する AU15b を再評価し、現在 revision・非 terminal・result-local certainty・検証済み非依拠判断・Task-wide Unknown barrier を含む全条件が成立した場合だけ採用・完了します。条件不成立なら記録は保持して未採用のままとし、provider / Action / runner はどちらの場合も 0 起動です。
10. **失われてはならない情報**: 結果 identity と安全な本文または body-free held の 1 回だけの到着 record、依拠リビジョンと委任対応（delegation = execution lifetime）、final result 到着による seal と seal 時点の authoritative set membership、Action 試行相関の authoritative set 検証結果、completion commit 時点で同じ `TaskId` の `Unknown` が 0 件であること（Task-wide completion barrier の不変条件）、採用リビジョン、現在の progress、terminal 後の新規開始拒否、terminal 後に遅延した結果の帰属、到着済み・採用未確定の状態。
11. **multi-turn + multi-action（複数 inference turn と Action の束）**: 1 delegation D1 は 1 つの delegated Task Agent execution であり、複数の inference turn を持ち得ます。T1 が A1（List）と A2（Read）を開始し、T2 が A3（Create）を開始し、T3 が final answer を提出したとします。finalization 境界で X を record すると D1 が seal され、authoritative set(X) は delegation D1 の全 Action 試行 {A1, A2, A3} です。T3 の inference attempt identity だけから列挙して `{}` や `{A3}` にはしません（source inference attempt identity と execution lifetime を混同しません）。
12. **action-before-seal race**: A1 の AU5 commit が result X の AU15a より先に成立した場合、A1 は D1 の authoritative set に含まれます。AU5 と AU15a は同一 SQLite master 上の短い `Immediate` トランザクションで直列化されるため、seal 時点の membership が一意に決まります。AU15b は {A1} を列挙し、claim と一致すれば採用判定へ進みます。
13. **action-after-seal race**: result X の AU15a が D1 を seal した後に D1 の新しい Action A2 要求が届いた場合、AU5 は同じ不分区間で seal を検出して `ExecutionSealed` を返し、`action_attempt` を挿入せず外部作用も行いません（authoritative set は {A1} のまま増えません）。
14. **inference-after-seal**: result X が D1 を seal した後、D1 の新しい inference claim は AU14 が同じ不分区間で seal を検出して `TaskPremiseStale` として拒否し、作業側の再読込（`load_delegation_result(D1)`）が `ExecutionSealed { delegation: D1 }` へ写します。provider I/O は発生しません。Task が InProgress のままであっても、同じ D1 を再利用して work を継続することはできません。
15. **in-flight Unknown（seal 前開始の未確定 Action）**: A1 が seal 前に durable start し、確定度が `Unknown` のまま result X が D1 を seal し、AU15b が `WithheldByEffectFacts(A1)` を返して Task は `InProgress` のままとします。seal は既に start した work を「無かったこと」にしないため、A1 は authoritative set に含まれたままです。その後 ene-action owner の客観的証拠が A1 を `Unknown → ConfirmedSuccess` に進めると、同じ result X の AU15b を再評価してよく、membership は seal 時点の {A1} のまま、他の条件が揃えば完了が成立します（set membership と certainty update を混同せず、membership を増減させません）。
16. **second final result（同一 delegation の 2 つ目の final result）**: D1 が result X で seal 済みのとき、異なる `TaskResultId` Y の final arrival を D1 に対して試みても、AU15a の durable invariant（1 delegation につき final `task_result` は最大 1 行）により 2 つ目の行は作られず fail closed（技術的エラー）です。同じ X の retry は委任・依拠リビジョンの帰属と recorded なら本文の一致を確認して既存 disposition を返します（本文不一致は技術的エラー。erasure-held / body-erased は帰属だけを比較し、本文を戻さない）。既に seal 済みの execution に新しい final result を受理しません。
17. **new delegation after sealed execution**: D1 が result X で seal されても Task が `InProgress` のまま（例: `WithheldByEffectFacts`）であれば、現在の前提・policy が許す限り新しい delegation D2 を AU3 で作成できます（Task が terminal でないため拒否されません）。D2 は新しい execution lifetime であり、D1 の Action 試行集合を引き継がず、D1 を再利用して work を継続することもありません。D2 の final result は D2 の Action 試行集合に対して検証されます。
18. **cross-delegation unresolved Unknown（reviewer reproduction）**: current Task R1。D1 の A1 が `Unknown` のまま final result X を提出して D1 が seal され、AU15b は `WithheldByEffectFacts { attempts: [A1] }` を返して Task は `InProgress` のままです。その後 D2 が AU3 で作成され、D2 の AU5 が A2 を `Unknown` として durable start します（D2 は final result をまだ持たない unsealed delegation ですが、既に開始した試行は barrier の対象です）。A1 が ene-action owner の客観的証拠で `Unknown → ConfirmedSuccess` に進んだ後に X の採用を再試行しても、AU15b は同じ短いトランザクションで Task-wide completion barrier を検証し、同じ `TaskId` に属する A2 の `Unknown` を検出するため `Completed` にしません（blockers = result-local の `Unknown` / 非依拠を検証できない失敗 ∪ Task-wide の `Unknown` = `{A2}`）。Task は `InProgress` のままで、A2 の settlement を待ちます。
19. **cross-delegation Unknown settles**: 18 の続きで、A2 が `Unknown → ConfirmedSuccess` に進み、同じ `TaskId` に `Unknown` が 1 件も無くなった後に X の採用を再試行すると、他の条件（現在 revision・purpose identity・`task.progress` 非 terminal（`Cancelled` でない））が揃っていれば `Completed` が成立し得ます。`task_result_attempt(X, …)` に刻印されるのは X の result-local authoritative set（D1 の依存、例: `{A1}`）のままで、A2 を X の dependency として捏造しません。
20. **AU5-before-completion race**: D2 の AU5 が A2 を `Unknown` として先に commit した場合、後から届いた X の AU15b は同じ SQLite master 上の短い `Immediate` トランザクションで直列化され、Task-wide completion barrier が A2 を検出して `WithheldByEffectFacts` を返します。Task は `InProgress` のままです。
21. **completion-before-AU5 race**: X の AU15b が Task-wide completion barrier を通過して `Completed` を commit した後に D2 の AU5 が到着しても、`task.progress` が terminal であるため AU5 は `TaskTerminal` を返し、`action_attempt` を挿入せず外部作用も行いません（A2 の行は存在せず、完了後の外部作用も発生しません）。この 2 順序以外に「A2 が barrier に見えず、かつ terminal gate も通る」隙間は、AU5 と AU15b が同一 SQLite master 上の短い `Immediate` トランザクションで直列化されるため存在しません。
22. **settled cross-delegation success**: D2 の A2 が `ConfirmedSuccess` で settlement 済み、X の result-local 依存もすべて `ConfirmedSuccess`、同じ `TaskId` に `Unknown` が無い場合、A2 が X の dependency でなくても Task-wide completion barrier を理由に完了を block しません（`AdoptedAsCompletion` が成立し得ます）。完了 commit 後に新しい AU5 は terminal gate で拒否されるため、A2 の fact は durable に残ったままです。
23. **settled cross-delegation failure**: D2 の A2 が `ConfirmedFailure` で settlement 済み、X の result-local 依存がすべて `ConfirmedSuccess`、`Unknown` が無い場合、A2 が X の result-local dependency でなければ、unsettled-effect barrier だけを理由に完了を block しません（外部作用 certainty として settlement 済みだからです）。A2 が X の result-local set に含まれる場合は、`RefusedBeforeEffect` の確認と Task owner の代替達成・残作業・客観的根拠の検証に基づく非依拠記録がない限り採用を block します。
24. **同一 delegation の確認済み無作用失敗と代替達成**: D1 で A1 が `ConfirmedFailure` + `RefusedBeforeEffect`（対象への作用なしを Action owner が確認）、代替試行 A2 が `ConfirmedSuccess`。X の claim は必ず `[A1, A2]` であり、`[A2]` は欠如として技術的エラーになります。Task owner が現在の目的に対して A2 の結果と客観的な事後検証記録から代替達成を確認し、残作業なしと A1 に依拠しない理由を検証できれば、AU15b は同一の `Immediate` commit で A1/A2 の相関・A1 の非依拠判断と根拠参照・X の採用・`Completed` を記録できます。A1 の `certainty` / `grounds` は変更しません。事後検証が無い、残作業がある、A1 が部分的に作用した、または同じ Task のどこかに `Unknown` がある場合は保留します。クラッシュ後の AU15b 再評価も同じ条件を要求し、根拠の無い LLM の成功報告では採用しません。
25. **old revision Unknown（steering を跨ぐ started Action）**: current Task が R1 のとき D1 の A1 が `Unknown` で durable start し、owner の steering が R2 を確定し、その後 R2 の D-new が result X を提出したとします。steering は既に開始した外部作用を取り消さないため、A1 は R2 の completion 後に外部作用を確定し得ます。AU15b の Task-wide completion barrier は `task_id` 単位（全 revision / 全 delegation）で `Unknown` を検出するため、A1 が `Unknown` の間は R2 の X を `Completed` にしません。A1 が ene-action owner の客観的証拠で settlement した後に X の採用を再試行してよく、その時点で barrier が clear なら完了が成立し得ます。barrier を current `TaskRef` の delegation に限定しないのは、この old-revision started Action を漏らさないためです。
26. **検証後の path 差し替え**: Workspace 内の `safe/file` を解決して認可した後、別プロセスが `safe` を Workspace 外への symlink / reparse point に差し替えます。AU5 後に記録用 `RealTargetRef` の path を開き直して外部ファイルを Read / Edit することは許しません。保持した実対象にだけ作用できる場合はその対象を使い、同一性と境界を維持できない場合は拒否します。Create では検証済み親 handle から no-follow・no-clobber で leaf を作成し、競合で親や leaf の境界が不明なら拒否します。AU5 が既に commit 済みなら試行記録を残し、無作用を証明できない結果は `Unknown` です。

27. **AU15a credential-set race（Stage 6 C3）**: final answer を観測した revision C1 の `ScrubbedText` と C1 を X の到着実引数にした後、credential owner が C2 を確定したとします。AU15a が現在 C2 を先に読めば `StaleCredentialSet { current: C2 }` を返し、X の本文・seal・採用を 0 にします。実行側は元の final answer をメモリ上で C2 と D1 の全旧 used-version IDs の秘密に対して再 scrub し、同じ X で bounded に再提出します。AU15a が先に C1 で record / seal を確定していれば、C2 による同じ X の retry は委任・依拠 TaskRef の帰属と、recorded なら到着本文の一致を確認します。再 scrub で本文が変わった場合は技術的エラーとし、既存行を読み戻して元の帰属・disposition を説明します。body-free 行は帰属だけを比較し、いずれも Action を再実行しません。
28. **非依拠理由の事後消去（Stage 6）**: X の採用時に `VerifiedNonReliance { attempt: A1, reason: R, evidence_refs: [E1] }` を検証・記録した後、削除条件が R を覆う場合、Task owner は R を消去して `NonRelianceReasonDisposition::Erased` を刻みます。`load_task_result(X)` / `load_delegation_result(D1)` は `RecordedNonReliance { attempt: A1, reason: None, reason_disposition: Erased, evidence_refs: [E1] }` を返し、過去の採用・試行相関を維持します。E1 の参照先本文はその owner が消去し、X の本文や旧通知から R を再構成しません。

AU15a の追加検証では、D1 の複数の inference claims がそれぞれ異なる旧 version を使用した後に現行 version が更新された場合、final answer の scrub 証明が現行 revision と D1 の全旧 used-version IDs に結合されることを確認します。scrub と AU15a の間の新 claim で集合が増えた場合は本文・seal を書かず bounded に再 scrub し、必要な旧 OS item が読めなければ本文保存・提示を unavailable とします。`recorded` の同一 X に異なる scrub 後本文で再到着した場合は既存行を保持して技術的エラーとし、X の bounded read で既存の帰属・disposition を説明します。`erasure_held` / `body_erased` は帰属だけを比較し、いずれも外部作用を自動再実行しません。

### V-11 Task Agent 採用指示本文の解決と prompt 配線（H-A・K-E・CI §5.2・CCT §7.2・PR グループB/D）

※ provider 送信を伴う step（1・2・3・7・9・11・12）は、Stage 4 erasure-currentness foundation（canonical current-condition store を AU14 claim と同一トランザクションで照会する data-use currentness gate と `DataUseHeld`）が実装された後の契約です。Stage 4 の implementation slice 順は、foundation（slice A）を先に置き、本文の bounded read・source/role/companion 検証・`InstructionSourceMissing`・論理入力組み立て・scrub（slice B）を provider-send-enabled として foundation より先に merge しないことです（失敗経路 4・5・6・8・10 は foundation より前に実装できます）。Stage 4 時点では active covering condition は通常空であり、それを canonical store への照会で authoritative に確認して送信します。full Targeted Deletion producer（operation lifecycle / sweep / participant 消去 / 残存検証 / completion）は Stage 6 のままであり、Stage 4 はそれに依存しません。producer-less の「削除なし」前提を置いて送信経路を先行 enable してはなりません。

1. **one instruction（採用指示 1 件の正常系）**: Task は R1 で purpose P を採用済み。owner の steering が R2 を確定し、`AdoptedInstruction` E1（`origin.kind = OwnerConversation`、`origin.source = H1`）が R2 と同じ `forward_steering` トランザクションで記録されています。H1.text = I1。次の Task Agent turn は delegated execution D1（依拠 R2）に対して実行されます。ハーネスは `load_task` の context から E1 を取り出し、`TaskInstructionSource.load_owner_instruction({ OwnerConversation, H1 })` で H1（role Owner、companion = assignee）を解決し、source 一致・role・companion を検証します。論理入力「purpose P → instruction I1」を組み立てて `SecretScrubber` を 1 回通し、返却された `ScrubbedText` だけを `TaskAgentInference` へ渡します。AU14 claim は依拠 R2 を比較し、成功した場合のみ provider I/O が始まります。`task_context_entry` へ I1 は複製されず、History が正本のままです。
2. **multiple steering instructions（複数指示の順序維持）**: E1→H1（R2 採用）、E2→H2（R3 採用）、E3→H3（R4 採用、current）。`load_task` は purpose 項目の後に E1、E2、E3 を (reference.revision, entry_id) 昇順で返します。ハーネスはこの順序のまま I1、I2、I3 を論理入力へ含め、revision 番号だけで再ソートせず、History timeline 順へ置換せず、source ID で dedupe しません。3 件すべてが 1 回の scrub を通ります。
3. **duplicate source identity（同じ source の複数採用）**: 同じ History source H1 が別の採用機会に E1 と E2 として 2 回採用されている場合（canonical contract 上、同一 source の再提案は別の entry identity として採用され得ます）、ハーネスは E1→H1 と E2→H1 をそれぞれ解決し、context 順に I1 を 2 回とも論理入力へ含めます。source ID だけで dedupe して 1 件に畳んだり、entry identity を統合したりしません。
4. **missing History source（参照先欠如）**: AdoptedInstruction E1（origin.source = H1）が存在するが H1 行が durable に存在しない場合、`TaskInstructionSource` は `Ok(None)` を返します。ハーネスは `TaskAgentTurnOutcome::InstructionSourceMissing { entry: E1, source: H1 }` を返し、指示を黙って落として purpose だけで続行することも、本文を捏造することも、`TaskContextEntry` を削除・書き換え・retire することもなく、provider I/O を開始しません（scrub も inference claim も行いません）。この状態は破損とは限りません。origin.source は参照であり、参照先 record の存在は TaskContextEntry の validity 条件ではなく、将来の Targeted Deletion が History 行を消去した場合も同じ outcome で安全に停止します。ただし、bounded read の時点で H1 を読めた後に deletion が始まった場合は source absence ではなく data-use currentness refusal であり、`InstructionSourceMissing` へ偽装せず `DataUseHeld`（Case 11）として扱います。
5. **foreign Companion source（別 Companion の発言）**: Task の assignee が C1 なのに、H1 行の companion が C2 である場合、ハーネスは companion 一致検証で fail closed の技術的失敗とし、provider I/O を開始しません。C2 の Owner 発言を C1 の Task Agent prompt へ混入させることも、`InstructionSourceMissing` として扱うこともありません（対応関係の破損は技術的エラーであり、参照先欠如のドメイン判定へ丸めません）。TaskContextEntry は書き換えません。
6. **wrong History role（Owner 以外の発言）**: H1 行が `HistoryRole::Companion`（role = Companion）である場合、ハーネスは role = Owner 検証で fail closed の技術的失敗とし、provider I/O を開始しません。Companion の発言を Owner instruction として Task Agent へ渡すことはありません。
7. **steering race（body read 後の steering 勝ち）**: ハーネスが R2 current の context と採用指示本文（E1→H1）を解決し、論理入力を scrub した後、AU14 claim の前に concurrent steering が R3 を確定したとします。claim は依拠 TaskRef R2 と現在リビジョン R3 の不一致を同一トランザクションで検出して `TaskPremiseStale` を返し、作業側の再読込が `StaleTaskRevision { current: R3 }` へ写します。R2 の旧 prompt は provider へ送信されません。本文 read は claim transaction の外で行われ、その間 Task-wide mutex や長い SQLite transaction を保持しません（CCT §15.2 の await 中ロック保持禁止）。steering が R3→R4 とさらに進んだ場合も同じです。
8. **scrub failure（指示本文を含む論理入力の scrub 失敗）**: 目的本文と解決済み指示本文を組み立てた論理入力（採用指示本文を含む）の scrub が `SecretScrubError` で失敗した場合、ハーネスは `TaskAgentTurnError::InputUnavailable` を返し、provider I/O を開始しません。error / Debug / log に論理入力や指示本文を含めず、原文を保存しません。
9. **secret in instruction（指示本文内の credential-like value）**: H1.text に登録済み credential 値と同値の文字列が含まれていても、1 回の scrub がその出現を除去した `ScrubbedText` だけが inference port へ届きます。raw の I1 は provider へ渡らず、`TaskAgentInferencePremise` / `TaskAgentOutput` の Debug も本文を伏字化します。複数回 scrub した断片を組み合わせないため、credential-set revision の drift による adoption / commit の穴も作りません。
10. **reopen（再起動後の再解決と自動 replay 禁止）**: Task / History を close して reopen した後も、現在 TaskRecord の E1 と History H1 の対応は保持されており、同じ `origin.source` から同じ指示本文を再解決できます。すでに確定した inference attempt は durable fact のまま残り、provider 呼び出しの自動 replay も、前回 turn の prompt の自動再送も行いません。新しい turn は明示的な呼び出し（`TaskAgentTurnPremise`）に対してのみ、現在の TaskRef 前提を AU14 で再照合してから開始します。
11. **read → deletion → send admission（Case 11）**: Task R2 の AdoptedInstruction E1 → H1。Task Agent turn が E1 を読み、History bounded read で H1.text を解決し、目的本文と合わせた論理入力を scrub した後、provider 送信 admission（AU14 claim）の前に Targeted Deletion が開始され、H1 を覆う消去条件が durable になったとします（Task revision / consent / credential-set は変わらない）。AU14 は `data_use` の H1 を現在の消去条件と同一の不分区間で照合して coverage を検出し、送信を拒否します。期待: AU14 の TaskRef 一致だけでは不十分であること、H1 は read 時に存在したため `InstructionSourceMissing` にはならないこと（materialize 済み buffer を理由に「もともと無かった」へ偽装しない）、provider 呼び出しが 0 回・受領 0 バイトであること、prompt の retry / cache が残らないこと、`TaskContextEntry` が変更されないこと、outcome が data-use hold（`DataUseHeld`）であり `StaleTaskRevision` / `InputUnavailable` / 技術的エラーへ丸められないこと。採用目的 entry の `origin.source` も同じ `data_use` に含まれるため、purpose と instruction の間に gate の非対称はありません。
12. **send admission → deletion（Case 12）**: source-aware な送信 admission（AU14 claim）が先に確定し、attempt 行と `data_use`（H1）が durable になった後で Targeted Deletion が H1 を覆う条件を durable 化したとします。期待: deletion は確定済み attempt を「送信されなかった」ものへ書き換えません（実行事実を保持し、already-started use と未開始 send を attempt 行の durable 化の前後で区別します）。attempt は already-started use として削除参加に列挙され、ベストエフォート停止・縮小の対象になります。provider が遅延して返した結果は、受入境界で現在の消去条件と照合され、削除対象本文を現在 Task へ採用・再保存しません（body-free `ErasureHeld`（対象本文を含む場合）の帰属。確定度は改ざんしません）。現在 Task への採用・再保存は既存の削除 invariant に従います。
13. **naive re-read は不十分（Case 13）**: `History re-read が present を返す → 直後に deletion が開始 → provider send` という方式、および `History read → scrub → deletion status read → AU14 → provider` という方式は採用しません。re-read / check と send の間には同じ TOCTOU race が残り、その間に durable 化した消去条件を provider send がすり抜けるためです。送信を開始可能にする linearization point は AU14 claim の commit そのものであり、現在の消去条件の照合はその同一トランザクション内で行います。claim 前に deletion status を再読込する方式で代替してはなりません。
14. **Stage 4 / no active deletion（Case 14）**: Stage 4 erasure-currentness foundation が実装済みで、canonical current-condition store には送信する source を覆う active covering condition が 1 件も無い（authoritative empty set）とします。Task Agent は採用目的 entry と採用指示 entry の source を解決し、scrub に成功した後、AU14 が claim と同一の `Immediate` トランザクションで canonical current condition を照会してどの source も covered でないことを確認し、claim を commit して provider I/O を開始します。期待: Stage 4 でも Task Agent inference が実際に実行できること、`NoDeletion` sentinel を使わないこと、空の authoritative set だけで送信許可が成立すること、Stage 6 の full deletion implementation を要求しないこと。
15. **Stage 6 later activates deletion（Case 15）**: 同じ Stage 4 foundation が既に存在する状態で、Stage 6 の Targeted Deletion producer が operation を開始し、enforcement より先に active covering condition を既存の canonical store へ durable に記録したとします。その後 Task Agent の AU14 が到着すると、同じ pre-existing の coverage gate がそれを見て `DataUseHeld` を返し、provider は 0 バイトも受け取りません。期待: Stage 6 が Stage 4 の gate を置き換えないこと、別の currentness store / 別の source correlation を作らないこと、durable-before-enforce が維持されること。

### V-12 Task cancel の受付・admission gate・遅延結果（H-A・K-E・K-K・CCT §14）

1. **正常系（受理と停止完了の分離）**: current Task は R1 / `InProgress`、delegation D1 は R1 を依拠し、D1 の execution で inference turn T1 と Action 試行 A1（`Unknown`）が durable に記録され、provider 呼び出し・外部作用が進行中とします。個体調整（会話）または第一者管理経路からの中断要求を `cancel_task` が 1 つの `Immediate` トランザクションで受け、`task.progress` を `InProgress → Cancelled` へ CAS して `CancelAccepted` を返します。この commit だけで受理は完了し、以後の新規委任・steering・推論 claim・Action 開始・現在 Task への結果採用は既存の admission gate で拒否されます（already-started activity の事実記録は拒否しません）。同じ不分区間で D1 の行・T1 の attempt 行・A1 の attempt 行・`data_use` 相関・確定度を変更しません。進行中の provider I/O と外部作用の停止は Host がベストエフォートで試み、停止できなかったこと・既に作用したこと・`Unknown` はそのまま報告します。期待: `CancelAccepted` は停止完了を意味しないこと、受理は LLM / エージェントの終了を待たないこと、受理後に新しい外部作用が開始されないこと、`Unknown` が不変であること。
2. **冪等な再要求**: 1 の後、無関係な後続 Owner 入力が受理されても、同じ Task の中断を明示撤回・置換した認証済み Owner 指示がなければ、同じ `CancelTaskCommand` の再送に対して `cancel_task` は `task.progress = Cancelled` を同一トランザクションで検出して `AlreadyCancelled` を返し、書き込みも追加の受理 record も発生しません。受理は 1 回だけで、再要求で状態・事実を上書きしません。期待: 何度再送しても受理が重複せず、二度目の terminal transition も起きないこと。
3. **Completed / Failed との競合（single-master の順序）**: 中断要求と AU15b 完了確定が競合した場合、同一 SQLite master 上の短い `Immediate` トランザクションで直列化されます。(a) AU15b が先に `Completed` を commit した場合、`cancel_task` は `TaskTerminal { task, progress: Completed }` を返して書き込みません（完了を中断で巻き戻さず、cancel は付きません）。(b) `cancel_task` が先に `Cancelled` を commit した場合、AU15b は result 行と authoritative set の検証を行った上で `task.progress` が terminal であるため attempt 相関の記録に留め、`adopted_revision` を刻まず `RecordedToOriginalOnly` を返します。`Failed` も同じで、先に確定した terminal が勝ちます。期待: 同じ Task が `Cancelled` と `Completed` の両方になる順序が存在しないこと、敗者が勝者の lifecycle state（`progress`・`adopted_revision`）を書き換えないこと。なお AU15b が敗者の場合は、採用しない result の検証済み `task_result_attempt` 相関を記録した上で `RecordedToOriginalOnly` を返します（already-started activity の事実記録であり、敗者の誤書き込みではありません）。
4. **AU3 / AU4 との競合**: 中断受付と委任作成 AU3・方針指示 AU4 も同じ master で直列化されます。(a) 先に AU3 が commit した場合、delegation D2 は durable に残り、その execution は以後の AU14/AU5 が terminal gate（`Cancelled`）で拒否するため work を開始できません（行を削除・変更しません）。(b) 先に中断が commit した場合、AU3 は `TaskTerminal { task, progress: Cancelled }` を返して delegation を作成せず、AU4 は `TaskTerminal` を返して revision を前進させません。steering が先に成立していても、会話 source の帰属が成立し同じ Task の中断を明示撤回・置換した後続 Owner 指示がなければ revision 前進は取り消されず、その新しい revision に対する中断として残ります。明示撤回・置換が先なら `Superseded` です。期待: cancel は revision stale に丸められないこと、AU3/AU4 の敗者に書き込みがないこと。
5. **AU14（claim 済み inference）との競合**: (a) AU14 claim が先に commit した場合、attempt 行と `data_use` は durable に残りますが、それだけで provider I/O の開始や送信成功を確定しません。Cancel が provider first-byte より先なら最終 dispatch gate が旧 ticket を拒否し、0 byte と以後の送信不能を証明して初めて `NotSent` と予約 `Released` を同じ commit で確定します。証明できなければ元の claim・相関・予約と利用量 `Unknown` を保持し、自動再送しません。first-byte が Cancel より先だった場合だけ既開始送信を元の試行に帰属させ、provider 呼び出しの abort をベストエフォートで試み、遅延した provider 出力は finalization 境界で AU15a に渡り得ます（消去時は body-free held と seal）。(b) 中断が claim より先なら、AU14 は claim トランザクション内で `task.progress` の terminal を検出して `TaskPremiseStale` を返し、作業側の再読込が `TaskTerminal { task, progress: Cancelled }` へ写します。provider は 0 バイトも受け取りません。`data_use` の消去条件 coverage 判定（`DataUseHeld`）は既存の順序（タスク前提 → data-use）のまま変わらず、cancel は消去条件を変更しません。期待: claim 済み attempt の事実が cancel で消えないこと、cancel 後に provider への新規送信が始まらないこと、未送信を証明した予約だけが解放されること。
6. **AU5（開始済み Action）との競合**: (a) AU5 が先に commit した場合、`action_attempt` は `Unknown` のまま durable に残り、外部作用は実行中・実行済みであり得ます。その後の中断は確定度を成功・失敗へ書き換えず、停止試行も作用の取り消しを保証しません。証拠を得た ene-action owner だけが `Unknown → Confirmed*` の CAS を行い、その事実を報告します。(b) 中断が先に commit した場合、AU5 は `task.progress` が非 terminal でないため `TaskTerminal` を返し、`action_attempt` を挿入せず外部作用も行いません。期待: 停止要求の名目で新規アクションを開始しないこと、`Unknown` が維持されること。
7. **遅延 final result（cancel 後に到着）**: 中断が durable になった後に、開始済み execution の final result が到着した場合、AU15a は消去照合済みの本文または body-free held の identity を 1 回だけ durable に記録し、同じ不分区間で delegation を seal します（cancel は到着 record を妨げず、消去 held の本文は保存しません）。続く AU15b は `task.progress` が terminal（`Cancelled`）であるため、authoritative set と claim の完全一致を検証した上で attempt 相関を 1 回だけ刻み、`adopted_revision` を刻まず、本文を保持できた場合は `RecordedToOriginalOnly`、消去 held なら `ErasureHeld` を返します。Task は `Cancelled` のままで、安全な結果本文だけが元の execution の記録として読み戻せます。期待: 遅延結果が現在 Task の完了・採用へ昇格しないこと、消去対象本文は再保存されないこと、確定度が改ざんされないこと。
8. **restart（再起動後）**: 中断受付 commit 後に Host がクラッシュ・再起動しても、`task.progress = Cancelled` は durable に読み戻せます。再起動や再ロードで失われるのはローカルの Future・cancellation token・停止 handle・ベストエフォート停止の進捗だけであり、開始済みの provider 処理や外部作用が停止したとは推定しません。durable な `inference_attempt`（と `data_use` 相関）・`action_attempt` はそのまま維持し、開始済み Action の `Unknown` は不明のまま保持します。再起動後に新しい委任・steering・推論 claim・Action 開始・現在 Task への結果採用が始まることはなく、provider 出力の自動 replay・結果の自動再採用・タスクの自動再開を行いません。遅延して届いた result は元の execution・依拠リビジョンへの履歴帰属（本文許可時は `RecordedToOriginalOnly`、消去 held 時は `ErasureHeld`）に留めます。ユーザーへは保存された progress、確定済みの作用、`Unknown`、未完了事項を提示し、再実行の明示的な指示を待ちます。`Cancelled` の Task は再開せず（progress を戻さず、同じ Task の delegation を再利用せず）、再実行はユーザーの指示の下で新しい Task を作成し、その Task の下に新しい delegation を作成して行います。期待: cancel されたタスクが再起動で再開しないこと、ローカルの handle の消失を停止完了の証拠にしないこと。
9. **cancel は deletion / hold ではない**: 中断は progress の terminal 遷移だけであり、消去条件・`data_use` 相関・Action の確定度・未伝達メッセージを変更しません。既に claim 済みの provider send は already-started use のまま削除参加の対象になり、未 claim の送信は AU14 の既存 gate（タスク terminal または `DataUseHeld`）が拒否します。hold スライスが導入する安全保留は別の比較条件であり、cancel はその代役・代用にはなりません。復元を跨ぐ cancel 要求の失効や hold 中の受理可否は、保全・消去スライスが hold 前提と対で `cancel_task` の同じ比較点に追加し、cancel 専用の第 2 の gate を作りません。
10. **無関係なタスクへの非波及**: 1 つの Task の中断は、その Task の admission gate だけを terminal にします。同じ Companion が担当する別の Task、別 Companion の Task、Host 上の無関係な通常処理は停止・変更しません（Companion 停止など複数 Task の中断は、個体調整が各 Task へ個別の中断要求を発行した結果として表現します）。
11. **受理の必須テスト（実装スライス）**: store / task レベルの必須ケースは、会話 source の Owner role・Companion・source ID・accepted sequence・対象 Task の照合、同じ Task の中断への後続の明示撤回・置換だけを Superseded とする比較、曖昧な対象・意図の NeedsClarification と管理経路の認証済み source 照合に加え、(a) `Started → Cancelled` と `InProgress → Cancelled` の CAS、(b) `AlreadyCancelled` が書き込みなしであること、(c) `Completed` / `Failed` に対する `TaskTerminal` が書き込みなしであること、(d) 存在しない Task の `MissingTask`、(e) cancel と AU3/AU4/AU5/AU15b の競合で先勝ちが維持されること（`Immediate` トランザクションの直列化。AU3/AU4/AU5 の敗者は書き込まず、AU15b の敗者は `task_result_attempt` の検証済み相関のみを記録して `RecordedToOriginalOnly` を返す）、(f) cancel が `action_attempt.certainty`・`inference_attempt`・`inference_attempt_data_use`・`task_result` の既存行を変更しないこと（AU15b の履歴相関と Action certainty の CAS は already-started activity の記録として許可される）、(g) close / reopen 後に `Cancelled` が維持され、同じ Task の delegation を再利用した再開・自動再開をしないこと、です。Host レベルでは、cancel 後の Task Agent turn が provider I/O を起こさず `TaskTerminal { progress: Cancelled }` へ写ること、cancel 後に到着した final result が AU15a で record/seal され AU15b で本文許可時は `RecordedToOriginalOnly`、消去 held 時は `ErasureHeld` になることを確認します。
12. **失われてはならない情報**: 中断要求の受理（`progress = Cancelled` の CAS commit）と実際の停止完了の区別、request と completion の分離、already-started activity の事実記録（AU15a の到着 record/seal、AU15b の検証済み相関、Action certainty、利用量）を cancel が禁止しないこと、開始済み試行・`Unknown`・`data_use`・到着済み result の不変性、`Completed` / `Failed` との先勝ち順序と敗者の書き込み範囲、冪等な再要求、再起動後の非再開と再実行のための新しい Task・新しい delegation、無関係なタスクへの非波及、cancel を deletion / hold と混同しないこと。

13. **会話 source と新 Owner 入力の競合**: 会話起点の `CancelTaskCommand { task, source: OwnerConversation { intent } }` について、依拠 Owner 入力「止めて」の後に無関係な「状況も教えて」が AU16 より先に commit しても `CancelAccepted`（既受理なら `AlreadyCancelled`）であり、別 Task の発話も中断を撤回しません。依拠 message の accepted sequence より後の認証済み Owner 指示が**同じ Task の中断を明示撤回・置換**した場合だけ `Superseded`（書き込み・停止通知なし）です。対象・直接的中断意図・撤回が曖昧なら `NeedsClarification` で確認まで保留し、既受理の cancel を取り消したり同じ Task を再開したりしません。AU16 が先なら `CancelAccepted` が残り、その後の指示は中断を巻き戻しません。`FirstPartyManagement { source }` は管理操作の認証済み source・Owner 直指定と対象 Task の同一性を照合し、会話 currentness を要求しません。期待: Owner role・Companion・source ID・対象 Task と accepted sequence / Task-specific explicit supersession・未解決の確認待ちを型付き正本の PK / 索引付き限定範囲から同じ `Immediate` トランザクションで照合すること、モデル自己判断・未認証第三者の発言・元指示に帰属しない遅延 directive で cancel を mint しないこと。
14. **遅延 directive・曖昧な意図**: 「Task T を止めて」が認証済み Owner message M として受理され、無関係な発話後に M に帰属する遅延 `cancel` directive が届いても、AU16 は M に帰属して受理時に確定した intent の PK / accepted sequence / Task T の対応と、元 intent / Task に紐付く明示撤回・置換 fact の不在を再照合して受理します。同じ directive の重複は `AlreadyCancelled` で、別 message への付け替え・別 Companion の message・モデル自身や未認証第三者の発言に基づく directive は Task を変更しません。M の「止めて」がどの Task を指すか確定しない場合や、後続指示が撤回か単なる照会か判定できない場合は `NeedsClarification` で確認を求め、推測で受理も `Superseded` も確定しません。確認によって既に `Cancelled` の Task を非 terminal に戻したり、同じ Task の resume を許可したりしません。


15. **型付き正本・受理と復旧の必須検証**: 会話担当の明確な「止めて」は Owner message の受理と `OwnerTaskCancelIntent` の body-free 記録が同じ master に揃い、LLM / Agent を待たず AU16 へ届く。対象不明の入力は候補を伴う確認待ちとなり intent を発行せず、確認入力の受理で初めて Task に帰属する。後続が撤回か曖昧なら元 intent / Task に `PendingClarification` が付いて AU16 は `NeedsClarification`、Owner の確認結果を元 pending ID に結び、明示撤回なら `Superseded`、撤回でなければ cancel 可能とする。無関係な「状況教えて」には後続 fact が付かず cancel 可能。これらを受理と AU16 の間の crash、再送、History 本文 retention / erasure、復元後にも同じ outcome とし、別 Task / Companion への付け替え・provider 出力・未認証第三者からの fact 発行、欠損相関、全文走査・推測による補完を拒否する。`intent_id` PK と `(intent_id, kind, accepted_sequence)` と未解決 pending 部分索引の `EXISTS` / `LIMIT 1` だけで検証し、最終 accepted Owner message への一致を条件にしない。

## 18. 意図的に残した Design Freedom

- 各インターフェースの具体的なメソッド名、モジュール配置、同期／非同期の粒度（本書では `async fn` のシグネチャを提案していますが、アクターモデル、チャンネル通信、直接の関数呼び出しなどの低レベルな実装機構は固定しません）。
- 由来情報（provenance）、対象範囲（source_range）、選択前提（selection_premise）などの内部データ構造の具体的表現形式、確認ダイアログのまとめ方、権限評価エンジンの内部アルゴリズム、識別子の検証・鮮度確認・競合制御の具体的機構（CCT 第5節で示した3つの選択肢：楽観的CAS、担当者ローカルのメールボックス、短いDBトランザクション内での不可分な読み取り照合のいずれを採用しても構いません）。
- コンテキスト組み立て（Context Assembly）およびルーティング用コンテキストの生成アルゴリズム、データ形式、更新頻度、鮮度の判定基準、選択アルゴリズム、文章要約、ベクトル検索のスコアリング計算、プロンプトテンプレートの構造、キャッシュの実装方式。
- 費用予約量の具体的な計算式、集計期間、推定方式、内部リソースの配分方針、反復処理の抑制閾値、画面キャプチャのインターバル、イベント駆動とタイマー待機の具体的機構。
- ペアリングおよび初期設定における認証材料の具体的フォーマット、暗号化保護方式、受け渡しプロトコル、ホスト再起動後における元のクライアントへの再接続待機時間、ネットワーク切断の検知アルゴリズム、在席の調停手順、対話やアクションの安全な区切り処理、停止指示の伝達機構。
- 検索、無効化、検証の低レベルな実装手法、バックアップ作成時の整合時点の選定アルゴリズム、バックアップファイルの保存形式、復元時のマスター切り替え手順、暗号化アルゴリズム、検索用暗号トークン（`SealedSearchToken`）の具体的実装。
- 監査ログの出力フォーマット、診断情報およびテレメトリの収集スタック、画面提示確認のUIデザイン、未伝達要約の粒度、データの具体的な保持期間、UIの画面レイアウト。
- なお、上記の設計自由度があるからといって、システム全体を統制するような単一の巨大コンテキスト層、ポリシーエンジン、統合マネージャー、集中サービス、中央コーディネーターなどを後から無秩序に追加してはなりません。本書で定めた既存の責務分担、各領域の担当責任者（semantic owner）、ホスト／クライアント間の配置原則、および厳格な信頼境界を常に遵守しながら、具体的な実装方法を選択します。
