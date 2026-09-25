# Concurrency / Race Control / Stale-result Acceptance の具体設計 — Step 13 Concrete Design

このドキュメントは、ene における**「並行処理・競合制御・遅延結果の安全な受け入れ（Concurrency / Race Control / Stale-result Acceptance）」**に関する具体的な設計仕様書です。
前提として、[対応関係・識別（correspondence-identity.md）](correspondence-identity.md)で定めた識別子（ID）、リビジョン（revision）、世代（generation）、対応関係（correlation）、境界トークン（boundary token）の定義と、[データ永続化と復旧（persistence-recovery.md）](persistence-recovery.md)で定めた機能ごとの永続データ分類（D1/D2/D3/R/T/E）、原子性と更新順序（atomicity / ordering）、復旧ルール、事後可視化の原則（durable-before-visible）をそのまま引き継ぎます。
上位の設計原則との優先関係や矛盾が生じた場合の判断基準は、[設計文書 README](../README.md#設計文書の優先順位と信頼できる情報源) に従います。また、文中の「DR」は [依存関係ルール（dependency-rules.md）](../architecture/dependency-rules.md) を指します。

本書では各ドメインに適した制御メカニズムの選択指針を明確に定め、具体的な SQL 文、クレート分割、通信メッセージのバイナリ形式、リトライやタイムアウトの数値そのものは固定しません。

## 1. 対象と非対象

### 1.1 本書が具体化するもの

- **同時に起こり得る現象と競合の整理**: 何と何が競合し、競合したときにどちらを優先して受け入れるべきか（第3節）。
- **不可分に行うべき比較と直列化**: どこで一括比較（アトミックな検証）が必要で、どこで直列化（直列実行）が必要/不要か（第4節・第5節）。
- **古い結果（stale）や遅延到着した結果の検出・隔離**: 過去の前提で作られた結果を誤って現在の最新状態へ適用しないための仕組み（第5節・第6節）。
- **業務特性に応じた適切なメカニズムの選択**: すべてをひとつの方式で画一的に縛るのではなく、各ドメインの不変条件に適した方式を採用（第6節）。
- **各領域における競合制御**: タスク、外部作用（Action）、権限・利用コスト、滞在先（Presence）、個人データ完全削除、バックアップ復元、ファイルシステムとDBの境界、処理のキャンセル（第7〜12節）。
- **ロックの設計**: ロックの所有権、粒度、取得順序、および非同期待機（await）中にロックを保持してはならないルール（第15節）。
- **インターフェースが満たすべき並行特性**: 各機能の境界面で確実に引き渡すべき情報（第16節）。
- **Rust での実装指針**: 必要最小限の標準的な非同期プリミティブの選定と注意点（第17節）。
- **競合シナリオのウォークスルー**: 典型的な競合パターンにおける安全性の検証（第18節）。

### 1.2 本書が決めないもの

- データベース固有の詳細な `CREATE TABLE` 定義、インデックス設計、マイグレーションスクリプト、バキューム設定。
- クレートやモジュールの物理分割、プロセスやスレッドの配置、通信プロトコルの詳細なバイト表現。
- リトライ回数、タイムアウト秒数、ポーリング間隔、タスクスケジューリングアルゴリズム、埋め込み計算や類似度スコアリング、プロンプトの組み立て手順。
- 各サブシステムの完全な API 一覧（ただし、並行処理において「決して落としてはならない必須プロパティ」は第16節で固定します）。
- 暗号化アルゴリズムの詳細、アーカイブファイル形式、電子署名の仕様。

### 1.3 用語

- **前提（premise）**: 時間のかかる非同期処理を開始した時点で読み取った、当時の最新状態のコピーです。処理完了後の確定（commit）時に「前提が変わっていないか」を再照合するための境界トークンとして使います。これ自体が現在の正当な権限を持つわけではありません。
- **確定（commit）**: 非同期処理の結果を、永続データ、現在のタスク、画面表示、次のアクションへ反映するためのごく短いアトミックな処理区間です。長時間の重い処理そのものをこの区間に含めてはいけません。
- **直列化ドメイン（serialization domain）**: データの整合性を保つために、同じ順序でひとつずつ順番に処理しなければならない状態変更の範囲です。排他制御そのものが目的ではなく、「前提と最新状態を比較して安全に確定する処理（compare-and-commit）」を不可分に行うための境界です。
- **確定前照合（compare-before-commit）**: 最初に前提を読み取った後はロックやトランザクションを保持せずに重い非同期処理を実行し、結果を確定する直前に最新状態と突き合わせて、前提が崩れていなければ安全に受け入れるという設計パターンです。
- **鮮度落ち（stale）**: 処理を開始したときの前提が、現在のリビジョンや世代、権限ルール、保留状態、データ削除、復元などの最新状況と食い違ってしまい、無効になった状態です。単なる到着順や経過時間（TTL）だけで判断してはいけません。
- **遅延結果の元の活動への帰属（delayed result の帰属）**: 時間がかかって遅れて到着した結果を、それが本来実行された古い試行や世代の履歴として記録することです。「履歴として残すこと」と「現在の状態へ自動的に反映すること」は明確に区別します。
- **上限枠の仮押さえ（reservation）**: 並行して実行される複数の処理が「まだ利用枠が残っている」と同時に誤認して上限を超えてしまうことを防ぐために、処理の開始前に必要なコストやリソース枠をあらかじめ引き当てておく仕組みです。
- **公開保護（publication guard）**: ファイルシステム上でファイルを保存・リネームしたものの、データベースへのポインタ登録が完了するまでのごくわずかな間に、定期的な不要ファイル掃除処理（orphan cleanup）によって誤って削除されてしまうのを防ぐための短い保護の仕組みです。

## 2. 原則（並行処理における基本ルール）

1. **到着順だけで新しさを判断しない**: 通信の遅延などにより、後から到着したデータが最新であるとは限りません。到着順だけを理由に受け入れてはいけません。
2. **時計の時刻や有効期限（TTL）だけで鮮度落ちを判断しない**: 時刻は人間への説明や表示、スケジュール計算のために使い、データの現在性はリビジョンや世代番号、最新のルールや保留状態と正しく一致しているかで判定します。
3. **システム全体共通のグローバルな連番カウンターを導入しない**: すべての状態に共通する単一のカウンターやグローバルトランザクションIDを前提としません。各ドメインの独立性を保ちます。
4. **結果が不明な状態（Unknown）を勝手に成功や失敗とみなさない**: 通信タイムアウトやキャンセル受付、再接続、復元が発生したからといって、結果が確認できていない処理を勝手に「成功した」「失敗した」「実行されなかった」と書き換えてはいけません。明確な事実の証拠が確認できるまで `Unknown` を維持します。
5. **許可された新たな外部作用は独立した新しい試行（Attempt）として扱う**: 重複する可能性のある再送処理を「同一の試行が続いているだけ」とみなしてはいけません。許された新たな作用では元試行への相関と新しい試行IDを保持します。同じ要求の受付再送は既存事実を読み戻し、Backup 専用 Task の同じ Task / 手動管理バックアップの同じ source では新しい出力試行を認めません。
6. **遅延到着した結果は元の試行へ記録する**: 時間がかかって遅れて返ってきた結果は、元のアクションの記録として保存します。現在の状況がすでに変わっている場合、今の会話や次のアクションへ無条件に自動採用してはいけません。
7. **再接続・再起動・バックアップ復元を理由に古い要求を勝手に再実行しない**: 過去の指示や実行権限をそのまま自動で再開してはいけません。再開してよいかは必ず現在の条件やユーザーの意思と再照合します。
8. **過去の認可判断の記録を生きた許可として再利用しない**: 過去のログに「許可された」と残っていても、それは過去の記録にすぎません。これから実行するアクションは、必ず現在の権限ルールや利用条件と照らし合わせて判定します。
9. **以前解決されたプロバイダの通信経路を恒久的な権限とみなさない**: 一度決定されたプロバイダの割り当て経路を、そのまま独立した正当な権限として使い回してはいけません。
10. **クライアント側の自己申告を滞在権限のマスターとみなさない**: パートナーが今どの端末に滞在しているか（Presence）の正式な権限は、ホストPCが一元管理する帰属記録のみが持ちます。
11. **データベースのトランザクションを長時間保持しない**: 外部サービスとの通信やAIの推論など、時間がかかる非同期処理の実行中に、DBトランザクションやミューテックスを保持したまま待機してはいけません。
12. **過度な単純化を避ける**: 「すべてアクターにする」「すべて単一のミューテックスで止める」「システム全体を一つの巨大トランザクションで囲む」「分散合意プロトコルを導入する」といった極端な画一化は避けます。ホストPCがマスターデータを一元管理する現在のシンプルな構造を最大限に活かします。
13. **アトミックな上書き保存だけでは更新の消失（Lost Update）は防げない**: ファイルのアトミックなリネームやDBの1トランザクションによる書き換えは「中途半端な破損データの公開」を防ぐことはできますが、「複数の書き込み処理が同時に古い状態を読み取り、お互いの変更を上書きして消してしまう問題（Lost Update）」までは防げません。そのため、読み取り・変更・書き戻し（read-modify-write）を行う一連の区間には、適切な直列化（短いロックやトランザクション、単一ワーカーへの集約など）を設けます。

## 3. 何が同時に起こり得るか（並行の源泉と競合の組み合わせ）

ene はホストPC上で動作する非同期システムです。並行処理の発生源は、ホスト内部のタスク、システム全体の処理、および外部サービスや通信の遅延です。複雑な分散合意プロトコルは不要であり、ホストPCのマスターデータへの安全な確定順序と、古い結果の適切な隔離を徹底することが鍵となります。

### 3.1 並行の源泉

| 発生源 | 具体例 | 性質 |
|---|---|---|
| **ユーザー操作の並行** | 会話中の追加指示（steering）、キャンセル、権限の承認、ルールの変更、同意の撤回、個体の停止・データ削除・復元 | 新しいリビジョンや世代を生み出し、現在進行中の処理の前提を過去のもの（鮮度落ち）にする側です。 |
| **タスク・委任の並行** | 複数の独立したタスク、同一タスクから分岐した並列委任、スケジュール実行の到来、委任エージェントの同時並行作業 | 同一タスクの最新リビジョンに対して複数の処理が同時に結果を書き込もうとする競合が発生します。 |
| **AI推論・アクションの並行** | 複数プロバイダへの並行問い合わせ、フォールバックや再送、ツールやMCPの実行、Computer Use、長時間の推論処理 | 外部への作用はロールバック（巻き戻し）ができません。タイムアウトや応答紛失により結果が `Unknown`（成否不明）になり得ます。 |
| **クライアント端末の活動の並行** | 端末間の移動、ネットワーク切断、再接続、ホスト再起動後の復元、個体の一時停止、別端末からの同時呼び出し、遅延した古い端末メッセージ | 同一コンパニオンに対して同時に複数の端末がアクティブになろうとする競合や、過去の承認・一時状態の混入が発生します。 |
| **システム全域の重要操作** | 個人データの完全削除（Targeted Deletion）、バックアップ作成、リストア復元、データ初期化（Reset）、インデックス再構築、記憶の長期定着 | システム全体に新しい世代や時間区間を適用します。新規データの生成や再保存、古い生データの混入と競合します。 |
| **遅延・再送・障害復旧** | 遅れて届いた推論やツールの結果、遅延した利用量報告、プロバイダセッションの残存、端末側キャッシュの再送、クラッシュ後の孤立ファイル | 到着順が最後であっても現在の最新データではありません。元の活動の履歴として正しく整理する必要があります。 |

### 3.2 競合の組み合わせと受入方針（概要）

| 競合する組み合わせ | 安全に受け入れてよい側 | 受け入れてはならない側 |
|---|---|---|
| **会話の推論結果 vs ユーザーからの新しい追加指示・変更** | 新しい指示やリビジョンに基づく側を現在の会話として採用します。古い前提で生成された回答は、過去の思考の記録として元のタスク履歴に残すことができます。 | 古い前提で作られた回答を、新しい会話の確定結果として勝手に採用したり、次のアクションや画面表示へ無条件に引き渡したりすること。 |
| **タスクエージェントの結果 vs ユーザーからの追加指示・キャンセル** | 指示変更後の最新リビジョンと目的に合致する結果のみを採用します。古い指示の結果は元のリビジョンへの履歴記録に留めます。 | 過去の指示で作られた結果を新しい目標の達成として勝手に採用すること。キャンセル後に後続処理を自動開始すること。 |
| **並行して実行されるアクション試行** | 前提条件の確認とコスト枠の仮押さえをパスした試行は並行実行可能です。各試行の成否確定は試行ごとのアトミック更新（CAS）で個別に管理します。 | 成否不明の試行を勝手に「同じ試行が続いている」とみなして再送すること。古い承認や解決済み経路だけを根拠にアクションを開始すること。 |
| **アクションのタイムアウト vs 遅れて届いた成功応答** | 両方を元の試行履歴に事実として記録します。確定度の状態は新たな証拠によって `Unknown → ConfirmedSuccess` へアトミックに更新します。現在のタスクへ採用するかは別途最新条件と照合します。 | タイムアウトしたからといって `Unknown` を勝手に失敗や未実行に書き換えること。遅れて届いた成功結果を、状況が変わっているのに無条件で現在のタスクに採用すること。 |
| **並行する推論リクエスト vs 利用コスト上限（Cap）** | 事前に利用枠の仮押さえ（reservation）を完了したリクエストのみを実行します。仮押さえと確定の厳密な照合により上限の超過を確実に防ぎます。 | 同じ残枠を同時に複数のリクエストが奪い合って上限を突破すること。処理中や成否不明の利用枠を「まだ確定していないから」とゼロとして計算すること。 |
| **端末AからBへの移動呼び出し vs ネットワーク切断** | 世代番号（generation）の比較更新（CAS）で競り勝った単一の帰属先のみを有効とします。移動前の処理は安全な区切りまで完了させます。 | 同一コンパニオンが同時に2台の端末でアクティブになること。移行期間中に新旧どちらかの端末で新規処理を勝手に開始すること。古い端末の未完了処理を別の端末で勝手に引き継いで実行すること。 |
| **古い端末からの遅延再接続 vs 復元された滞在状態** | 現在の世代番号、最新の通信接続、権限、排他性がすべて確認できた側のみを有効とします。 | 古い一時キャッシュや過去の承認情報だけを根拠に接続を認めること。接続状況が確認できないのに「おそらく繋がっているだろう」と推定して処理を進めること。 |
| **新しい記憶の形成 vs 個人データの完全削除** | 削除条件、適用スコープ、保存禁止ルールに違反しない情報のみを記憶として定着させます。削除対象区間内に再生成・再到着した情報はすべて削除対象とします。 | 削除前の情報を参照している実行中の処理が、削除対象の情報を再保存してしまうこと。過去の根拠データだけを頼りに自動で再学習すること。削除の検証が完了していないのに完了と表示すること。 |
| **完全削除の完了 vs 遅延して届いた結果** | 削除完了後に新しく提供されたデータのみを保存対象とします。削除された過去の活動に紐づく遅延データは破棄するか、元のアクション履歴への記録に留め、再保存しません。 | 削除完了後に、過去のセッションやキャッシュから遅れて届いたデータを誤って再保存し、削除したはずのデータが復活してしまうこと。 |
| **バックアップ復元の切替 vs 復元前の旧実行結果** | 復元後の新しい世代番号を前提とし、復元後の保留状態や最新のセキュリティ条件を満たしたもののみを受け入れます。 | 復元前の古い環境で動いていた処理の結果が、復元後の新しいデータベースに混ざってしまうこと。復元が完了したことだけを理由に、保留中の外部処理をユーザーの確認なしに自動再開すること。 |
| **ファイル保存とDB登録の境目でのクラッシュ** | 「ファイル永続化 → 公開保護 → DBポインタ登録 → 画面への可視化」の順序を守り、どちらか片方しか存在しない不整合データはマスターデータや成功として扱いません。 | DBにレコードがあるのにファイルが存在しない、あるいはファイルはあるがDBにない状態を正常データとみなすこと。保存途中でリネーム直後のファイルを定期クリーンアップが誤って消去すること。 |

## 4. 直列化の単位（Serialization Domain）

### 4.1 方針

- 直列化（順番にひとつずつ処理すること）は、「前提条件の確認と確定処理を不可分（アトミック）に行う」ためにのみ設けます。お互いに影響しない独立したドメイン同士は完全に並行して実行させます。
- AIの推論やツールの実行、バックアップのステージング検証など、時間のかかる重い処理は直列化の区間の外で非同期に実行します。直列化が必要なのは、結果をDBやメモリに書き込んで確定する最後のごく短い瞬間だけです。
- 異なる機能にまたがる更新を、ひとつの巨大なトランザクションやすべてを司る単一のアクターにまとめ上げてはいけません。必要なのは「同一の短い SQLite トランザクション内で複数のテーブルを一括して安全に読み書きすること」であり、各機能の責任境界を曖昧にしてはいけません。

### 4.2 直列化ドメイン一覧表

| ドメインID | 直列化の単位 | このドメインに含める状態変更 | 並行して実行してよい処理 | 備考 |
|---|---|---|---|---|
| **SD-Task** | タスクごと（`TaskId`） | 追加指示の適用（新リビジョンと新コンテキストの前進）、委任エージェント作成の受付（progress の `Started → InProgress` を含む）、中断要求の受理（progress の `cancelled` への CAS。AU16）、結果の到着 record と delegation の execution seal、採用判定、Task progress の terminal への CAS（完了・失敗・中断）。AU3/AU4/AU14/AU5 の同一不分区間で非 terminal を必須とする terminal admission gate と、AU14/AU5 の同一不分区間で must-not-be-sealed を必須とする execution seal gate も含む | 異なる `TaskId` のすべての処理。同一タスクであっても読み取り専用の参照、進捗表示、思考ログの取得 | タスクの達成判定、外部作用の確定、権限の確認はそれぞれ別個の責任であり、ひとつの更新に混同しません。progress は revision とは別軸で、terminal 状態は吸収的です。terminal（`Completed` / `Failed` / `Cancelled`）は専用の `TaskTerminal` outcome で拒否し、revision stale へ丸めません。execution seal（final result 到着で閉じる 1 delegation の lifetime）は Task の terminal とは別の gate であり、Task が InProgress でも成立します（`ExecutionSealed`）。採用判定（AU15b）は、同じ不分区間で Task-wide completion barrier（同じ `TaskId` の全 revision / 全 delegation の `action_attempt` に `Unknown` が無いこと）を検証してから `Completed` へ CAS し、1 件でも `Unknown` が残れば完了しません。会話起点の Task 作成・steering・resume は依拠 Owner message が同じ master 上で最新の accepted Owner input かを同じ不分区間で比較します。cancel だけは会話担当が受理時に確定した body-free `OwnerTaskCancelIntent` を PK で照合し、元 intent / Task に結ばれた後続の型付き明示撤回・置換がある場合だけ `Superseded`（書き込みなし）にします。無関係な後続入力・別 Task の指示は撤回ではなく、曖昧なら `NeedsClarification` で確認まで保留します。第一者管理経路は認証済み source と対象 Task を照合し、会話 currentness を課しません。 |
| **SD-Attempt** | アクション試行ごと（`ActionAttemptId`） | 試行の開始（委任対応・依拠タスクリビジョン・現在のタスクリビジョン・現在の `workspace_assoc`・委任 `scope_assoc`・未使用の `relied_evaluation` の照合付き挿入）、確定度の更新（`Unknown → Confirmed`、証拠不能時の `Unknown` 据え置き）、停止要求・停止結果・保留（hold）の記録 | 異なる試行の実行や結果報告。同一試行の読み取り | 試行履歴は追加専用とし、行ごとの比較更新（CAS）で管理します。Action 評価 ID と試行 ID は single-use です。タスクの達成更新は別トランザクションで行います。 |
| **SD-Presence** | コンパニオンごとの滞在帰属（`CompanionId`） | 滞在先帰属の遷移（`旧端末 → 移行中 → 新端末`）、世代番号のインクリメント、アクティブ端末の切り替え、復旧ヒントの更新 | 異なるコンパニオンの滞在処理。同一コンパニオンであっても表示、音声再生、画面観測などの非帰属処理 | 端末との通信疎通確認はDBの外で行います。移行期間中は新旧どちらの端末でも端末依存の新規処理を開始しません。 |
| **SD-Cap** | コスト上限ごと（`CapId`） | 利用枠の仮押さえ（reservation）の登録と上限チェック、確定時の仮押さえから実績への振り替え、不要枠の解放 | 異なるコスト上限の消費。同一上限の読み取りや集計表示。仮押さえ完了後の推論処理そのもの | 同一トランザクション内でチェックし、処理中や成否不明の枠を勝手にゼロとみなしてはいけません。 |
| **SD-CharApply** | コンパニオンごとのキャラ適用（`CompanionId`） | 期待する静的定義 revision と現在の適用関係の版・内容、選択操作 identity と適用内容・由来の current erasure condition / provenance cut の照合、適用ポインタ・版の更新と履歴追加 | 異なるコンパニオンへの適用。新しいキャラクター定義自体の追加登録 | Character の静的定義と個体調整の適用関係は所有者を分離し、既存適用が参照する改訂・アセットを保持します。 |
| **SD-Undelivered** | 未伝達メッセージごと（`undelivered_id`） | 伝達状況の更新（`Pending → PresentationUnknown → Presented`）。会話・Task・Action 由来のいずれも親 fact と同一の短いトランザクションで参照を登録（PR §4.6） | 異なる未伝達メッセージの処理。同一メッセージの読み取りや要約の生成処理 | 画面や音声でユーザーに伝達されたことが確認できるまで「伝達完了」にしてはいけません。 |
| **SD-Deletion** | 削除操作ごと（`DeletionOperationId`）および全域スキャン | 削除操作と消去条件の先行永続化、各参加コンポーネントの完了集約と残存検証、検索用トークンの完全消去、全域完了のアトミック確定 | 各コンポーネント内部での局所的な削除作業や検証。削除対象外の通常業務。異なる削除操作同士 | 削除条件を先に永続化してから削除を進めます。検索トークンが復元可能な間は完了とみなさず `finalizing` 状態を維持します。 |
| **SD-Restore** | バックアップ復元の切替（単一の `restore_generation_state`） | ステージング検証完了後の復元世代番号の更新と、マスターポインタの切り替えの瞬間のみ | ステージング領域での検証作業、通常データの読み書き（切替の瞬間を除く）、バックアップファイルの作成、派生データの再構築 | 切替前は復元前の正常データがマスター、切替後は復元データがマスターとなり、中途半端な混合状態を作りません。 |
| **SD-RuleConsent** | ルール・同意ごと（`RuleId` / `AssignmentId` など） | ルール本文、解釈、スコープ、ユーザー同意リビジョンの更新、取り消し履歴の記録 | ルールの評価（読み取りと照合）そのもの。異なるルールや同意の変更 | 権限の評価処理とルールの更新処理は分離し、過去の許可ログを安易に生きた許可として使い回しません。 |
| **SD-CompanionLife** | コンパニオンごとの生存周期（`CompanionId`） | 実行中・一時停止・完全削除（削除マーカー情報）の遷移、新規処理を停止させる保留の先行設定 | 異なるコンパニオンのライフサイクル。各コンポーネント内での局所的な削除や検証 | 単一の巨大トランザクションで囲むのではなく、各機能の局所的な永続化を集約して成立させます。 |
| **SD-ScheduleOccurrence** | `(ScheduleId, 発生キー)` ごとの到来、`(ScheduleId, schedule_version, interval_id)` ごとの区間 cursor | 証明済み停止の `Missed`、今回の連続稼働中の AU2 と参照を伴う `Started`、crash 前後の停止未証明回の `Unclassified` を、区間種別・個体状態・Host / restore hold と不可分に比較・確定（PR AU18） | 異なる予定回の日時計算、権限準備、確定済み Task の実行・中断試行 | 個体 Stop / Deletion、Host 明示停止・startup、版変更は同じ SQLite master の短い比較点で到来・cursor と直列化します。現行版の連続稼働中の遅延 due は bounded な keyset で受付します。未来回は開始せず、旧版は分類専用です。停止下端のない crash を前回の分類済み境界から Missed と推定しません。メモリ上の scheduler mutex は権威になりません。 |

> [!NOTE]
> **直列化の対象外（完全に並行して実行してよい処理）**
> AIの推論実行、外部ツールやMCPの呼び出し、プロバイダセッションの維持、埋め込みベクトルや検索インデックスの再構築、プロンプト用コンテキストの組み立て、要約文の生成、画面表示や統計の集計、画面監視（Observer）のキャプチャや候補検知、バックアップファイルのコピー作業、復元用ステージングの整合性検証などは、直列化のロックを持たずに並行実行します。これらは処理開始時の「前提」を保持した状態で実行し、最後の書き込み時に「前提が崩れていないか」を照合します。

## 5. 確定前照合（Compare-before-commit: 前提と最新状態の比較）

### 5.1 基本的な処理の流れ

時間のかかる非同期処理においてデータベースのトランザクションを長時間ロックし続けないために、以下の「前提確認 → 非同期実行 → 確定直前照合」の流れを徹底します。

```text
1. 前提（premise）を読み取る（ごく短い読み取り。ロックやトランザクションは保持しない）
   - 対象のIDと期待されるリビジョン・世代番号
   - 根拠となる権限評価結果、ルールや同意のリビジョン、端末、コスト上限、保留状態
   - クライアント依存の処理であれば、申告された世代番号と現在の接続状況
   - 個人データの消去条件や復元世代の前提
   - タスクであれば目標や指示内容、委任範囲、ワークスペースの有効性。アクションであれば具体的な操作対象と種別

2. 非同期処理を実行する（ロックやトランザクションは一切保持しない）
   - AI推論、ツールの呼び出し、ステージング検証、派生データの生成など。
   - この実行中に、ユーザーからのキャンセル、追加指示、権限失効、データ削除、復元が発生する可能性があります。

3. 確定（commit）の直前に、最新状態と前提を比較する（ミリ秒単位のごく短いトランザクション）
   - SQLite の Immediate トランザクションを開始し、最新状態を再読込して前提と突き合わせます。
   - このトランザクションの中で非同期の await や外部通信を行ってはいけません。比較と書き込みのみを行います。

4. 整合性が保たれている場合のみ安全に受け入れる
   - 一致している場合: データを永続化し、画面や次のアクションへ可視化します。
   - 不一致の場合: 処理を保留（hold）、拒絶（deny）、または最新状態での再評価へと安全に倒します。
   - 遅延して届いた結果の場合: 現在の会話には勝手に反映せず、元の試行やタスクの履歴としてのみ記録します。
```

「最新の値を読んだはず」「キャッシュにヒットした」「到着順で最後だった」といった理由は、結果を受け入れてよい根拠にはなりません。

### 5.2 アトミックに突き合わせるべき条件一覧

| 実行しようとする処理 | 確定直前に不可分（アトミック）に比較するもの | 不一致だった場合の安全な扱い |
|---|---|---|
| **タスクへの委任・追加指示・アクション開始** | `(task_id, 期待されるtask_revision)` × 現在のタスクリビジョン × 現在の `task.progress`（非 terminal） × delegated execution の seal 状態（AU14/AU5 は not sealed） × 委任スコープ × ワークスペースの有効性 × 追加指示の前提（委任スコープ・ワークスペース・追加指示の前提を変更できる producer が存在するスライスが、同じ不分区間の比較に加えます。AU3 の作成スライスでは期待リビジョンと現在のリビジョンの比較照合が現在性を確定し、スコープは依拠したタスク単位から写した写しを保存します。Task Agent の推論開始も、委任の対応関係と依拠タスクリビジョンの比較、非 terminal な progress、およびその delegation が seal 済みでないことを推論試行の確定（attempt claim）と同一の不分区間で行います。Action 開始は委任（execution lifetime）を AU5 で照合します） | 処理を開始せず、最新状態での再評価に戻します。terminal（`Completed` / `Failed` / `Cancelled`）は再評価しても開始できないため `TaskTerminal` として区別し、`Stale*` へ丸めません。seal 済み delegation は Task が InProgress でも再開できないため `ExecutionSealed` として区別します（terminal と混同しません）。 |
| **タスクの中断受付（Cancel、AU16）** | 対象 `TaskId` × source（会話起点は `OwnerTaskCancelIntentRef` と元 intent / Task に結ぶ型付き後続 fact、第一者管理経路は認証済み操作 source / 対象 Task。会話 currentness 不要）× 現在の `task.progress`（非 terminal か）。会話の source 帰属・直接的中断意図は intent PK で、後続の同じ Task の撤回・確認待ちは元 intent の索引付き限定範囲で、progress CAS と同じ SQLite master の不分区間に比較します。History 全文走査や LLM 推測は行いません。曖昧なら確認まで保留します。revision・purpose の前提は比較しません（中断はタスク単位の操作であり、同時 steering と競合しても revision stale にしない。会話 source の `Superseded` は別） | 非 terminal（`started` / `in_progress`）のときだけ `cancelled` へ CAS し、commit を受理の durable marker とします（停止完了は判定・待機しません）。`completed` / `failed` は `TaskTerminal`、既に `cancelled` は `AlreadyCancelled`（冪等）、識別子不在は `MissingTask`、同じ Task の中断を後続の認証済み Owner 指示が明示撤回・置換した会話要求は `Superseded`、対象・意図・撤回が曖昧なら `NeedsClarification` として書き込みません。管理経路は会話の `Superseded` を受けません。 |
| **タスク結果の記録・採用・完了確定** | 結果 identity・安全な本文または body-free held の 1 回だけの到着 record と同一不分区間での delegation の execution seal（1 delegation につき final result は最大 1 つ）× delegation 行から解決する依拠 `(task_id, task_revision)` × 現在の `task.revision` と purpose identity（本文一致は使わない）× 現在 `task.progress` の非 terminal（`Cancelled` は terminal であり、cancel 専用の追加条件は置かない）× delegation（execution lifetime）から列挙した `action_attempt` の authoritative set と claim の完全一致（欠如・追加・重複は技術的エラー。membership は seal 時点で固定）× 各試行の確定度（`Unknown` は完了不可。確認済み無作用失敗は Task owner の代替達成・残作業・客観的根拠の検証と非依拠判断の durable 記録がある場合のみ許容。部分的作用・不足は保留）× Task-wide completion barrier（同じ `TaskId` に属する全 revision / 全 delegation の Action 試行に `Unknown` が無いこと） | 現在 revision が前進済み、または `task.progress` が terminal（`Completed` / `Failed` / `Cancelled`）なら現在へ採用せず `RecordedToOriginalOnly` として元の依拠リビジョンへ attempt 相関を記録します。blockers（authoritative set の `Unknown` / 非依拠判断を検証できない失敗 ∪ 同じ `TaskId` の全 revision / 全 delegation の `Unknown`。重複は 1 回）が空でなければ `WithheldByEffectFacts` として現在 Task を完了せず、result-local の attempt 相関のみ記録します（barrier で見つけた試行は `task_result_attempt` に刻印しません）。cross-delegation / 旧 revision の `ConfirmedSuccess` / `ConfirmedFailure` は barrier だけを理由に block せず、inference attempt と unsealed delegation の存在も barrier に含めません。seal 後に確定度が客観的証拠で進展した場合は同じ result の再評価で完了が成立し得ます（membership は変えません）。claim の欠如・追加・重複は部分的な単位として技術的エラーにし、stale / Withheld へ丸めません。同じ result ID の再 arrival は本文を増やさず同じ判定を再評価し、二度目の terminal transition を行いません。 |
| **外部アクションの新規開始** | 試行が前提とする `(タスクリビジョン, 委任（execution lifetime）とその seal 状態, 操作対象と種別, 依拠した権限評価)` × 現在の最新条件 × 現在の `task.progress`（非 terminal）× delegated execution が not sealed × クライアント依存なら滞在世代と接続状況 × 消去・復元の保留状態（Workspace 内ファイルシステムのスライスでは、委任対応 × 依拠タスクリビジョン × 現在のタスクリビジョン × 現在の `task.progress` × 現在の `workspace_assoc` × 委任 `scope_assoc` × 未使用の `relied_evaluation` × その delegation の final result 行が存在しないことを `action_attempt` 挿入と同一の不分区間で比較・保存します。依拠権限評価は K-B.1 の single-use な Action 評価 ID で、同じ ID での二度目の開始は拒否します。在席・消去・復元の比較は、それぞれの producer を持つスライスが同じ不分区間に加えます） | 処理を開始しません。terminal は `TaskTerminal`、seal 済みは `ExecutionSealed`、その他の前提不一致は `StalePremise` として区別します。ユーザーの確認待ちが必要な場合は待機状態にします。 |
| **アクション確定度の更新** | `(attempt_id, 期待される確定度=Unknown)` × 事実確認が取れた新たな証拠（実行・拡張自身による対象の事後確認。エージェントの自己申告は証拠にしない） | 確実な証拠がない限り書き換えません。確定済みの値からの更新は行いません。現在のタスクへの採用可否は別個に判定します。 |
| **クライアント依存活動の開始・継続** | 端末が主張する世代番号 × ホストが管理する現在の滞在世代番号 × 現在の接続・可用性 × 最新の権限や保留状態 | 接続や状態が確認できない場合は処理を継続しません。 |
| **削除区間中のデータ受入・生成・再保存** | 届いた・生成された情報の `(情報源との関係, 取得日時)` × 進行中の消去条件 `(operation, sweep, valid_interval)` | 削除区間に該当するデータはすべて消去対象とし、実行中の処理が誤って再保存しないようにブロックします。削除完了後に届いた古い関連データも再保存しません。 |
| **バックアップ復元後の処理利用** | 処理が前提とする `(復元世代番号, プロバイダ同意リビジョン, 認証情報, 依拠ルール)` × 現在の `(復元世代, 最新DB, 最新制約, 復元後保留)` | 復元前の古いセッションや同意だけを根拠に外部通信やタスクを自動実行しません。 |
| **権限・ルール解釈の採用** | 過去の許可記録・復元ルール・コンテキスト内の許可文 × 現在の `(ルールリビジョン, 同意, 端末条件, コスト上限, 失効・停止・消去・復元保留)` | 条件が変わっていれば過去の許可をそのまま適用せず、制御を変更しません。 |
| **利用コスト・リソース枠の継続判定** | 消費の `(用途・送信先, 報告済み/不明/処理中の区分)` × 現在の上限枠やリソース状況 | 処理中や成否不明の枠を勝手にゼロとみなさず、並行処理による上限超過を防止します。 |
| **キャラクター定義の適用** | `(character_id, expected_character_revision)` × 確定済み部品と資材 × `expected_applied_current`（Companion の適用 current の版・内容。未適用も含む）と実際の current × `OwnerSelectionRef` の操作 identity と対象・選択部品・両 premise の対応。適用候補の本文・資材・由来を比較点の外で消去条件と機械的に照合し、同じ点で候補同一性・現行 condition generation / phase・完了後 cut を削除開始・finalizing・completion と直列化します。 | 被覆あり・判定不能・消去条件の変化は `DataUseHeld` として適用・履歴追加を保留します。期待する定義または適用 current が違えば、現行適用状態を返して再選択を求め、適用・履歴追加は行いません。同じ操作 identity の別内容への付け替えや二重適用を拒否します。確定した適用が参照する改訂・管理下アセットは参照が続く間保持します。 |
| **全域削除の完了確定** | 各コンポーネントの完了状況 × 実際の残存検証 × 削除区間内に再到着したデータの取り込み × 検索用トークンの完全消去 | 未確認や失敗、検索トークンの残存がある場合は完了とみなさず、未完了（`finalizing`）を維持します。 |

### 5.3 単一のパイプラインとして強制しない

上記は「安全のために必要な比較」を整理したものであり、すべての処理に同一の重厚なパイプラインを強制するものではありません。各ドメインの特性に応じて、以下のいずれかの軽量で確実なメカニズムを選択できます：

- **楽観的比較更新（Optimistic CAS）**: タスクリビジョン、滞在先世代、キャラクター適用、確定度更新、コスト枠の仮押さえなどで標準的に使用。
- **機能ごとのローカルメールボックス（Owner-local mailbox）**: 同一タスクの指示順序、同一コンパニオンの滞在先切り替え、削除処理の完了集約などで順序を保証。
- **短い DB トランザクション内のアトミック読み書き**: コスト上限チェック、委任の受付、アクション試行の受付などで使用。

いずれの方式であっても、「前提を読み取って重い処理を行い、確定する直前の一瞬だけ最新状態と比較する」という原則と、「トランザクションの中で非同期の await を行わない」というルールを厳格に守ります。

## 6. ドメインごとの並行処理方式の選択（Concurrency Model）

単一のホストPC上で動作し、単一の SQLite ファイルと Tokio の非同期ランタイムを使用することを前提として、各ドメインの不変条件に最適なメカニズムを選択します。システム全体にひとつの画一的な方式を無理に押し付けることはしません。

| ドメイン | 選択するメカニズム | 選定の理由と、あえて採用しない選択肢 |
|---|---|---|
| **タスク / 追加指示 / 委任受付 / 中断受付** | タスクごとの局所的な直列化 ＋楽観的リビジョン比較（CAS）＋短い Immediate トランザクション。局所直列化は、同一プロセス内で同じ Task のタスクリビジョンまたは現在値を更新する producer が存在するスライスでは per-Task mailbox / per-Task Mutex として導入し、それまでの単一 producer 段階では単一の書き込み接続による短い Immediate トランザクション内の比較照合が同じ現在性保証を担います。AU3/AU4/AU14/AU5 は同じ比較内で非 terminal な `task.progress` を必須とし、AU14/AU5 はさらにその delegation の execution seal（final result 行の不存在）を必須とします。中断受付（AU16）は revision・purpose を比較せず、会話起点なら body-free intent PK と元 intent / Task に結ぶ後続の型付き撤回・確認待ちを同じ master の索引付き限定範囲で照合し（管理経路には会話比較を課さず）、現在の `task.progress` が非 terminal のときだけ `cancelled` へ CAS します（受理 = commit。停止完了は判定しません） | タスクの進行ポインタを正しく前進させるためです。システム全体を止めるグローバルロックは不要であり、異なるタスク同士は完全に並行動作できます。メカニズムの選択は Design Freedom であり、完了確定・結果採用など他の SD-Task メンバーの比較条件はそれぞれの producer スライスが定めます。terminal と execution seal の拒否は全 producer が同じ不分区間で行い（`TaskTerminal` / `ExecutionSealed`）、cancel は `task.progress` の terminal 値（`cancelled`）として同じ非 terminal gate が拒否し、cancel 専用の比較条件を追加しません。 |
| **アクション受付・確定度管理** | 追加専用の履歴記録（append-only insert）＋試行ごとの確定度比較更新（CAS）＋短いトランザクション。実行自体は非同期並行 | 試行は一度起きた歴史的事実であり現在の可変状態ではないため、全体的な順序制御は不要です。エージェントの自己申告を鵜呑みにしない安全策はコード上で担保します。 |
| **権限・ルールの評価** | 評価処理自体はスナップショット読み取りによる完全並行。確定時（予約や開始）のみ短いトランザクションで現在条件と再照合 | 評価処理の並行性をロックで潰さないためです。過去の許可キャッシュを生きた許可として誤認しないようロジックで制御します。 |
| **利用コスト・リソース枠** | 仮押さえ（Reserved）→ 確定（Committed）/ 解放（Released）の3状態管理＋短いトランザクション内でのアトミックチェック。仮押さえ完了後の推論はロックなし並行 | 単純な読み取りだけでは並行リクエストによる上限突破を防げないため、事前の引き当てが必須です。ただし推論中までロックを抱え込むことは避けます。 |
| **滞在先帰属（Presence）** | コンパニオンごとの直列化（帰属アクターまたは短い Mutex）＋世代番号比較更新（CAS）＋`旧→移行中→新` の確実な永続化遷移 | 同一コンパニオンが同時に2台の端末でアクティブになる事故を完全に防ぐため直列化が必須です。異なるコンパニオン同士は並行して処理できます。 |
| **キャラクター定義の適用** | コンパニオンごとの適用 current 版の CAS ＋確定済み定義 revision・選択操作 identity の同一短期トランザクション照合 | 同じ適用状態から並行して異なる部品を選択しても後着要求が先着の選択を上書きしないためです。新しいキャラクター定義の追加自体は並行して行えます。参照中の静的アセットは通常整理で除去しません。 |
| **未伝達メッセージ** | 登録は親イベントと同一トランザクション、伝達確定は行ごとの比較更新（ユーザーへの提示確認後のみ）。要約生成は並行 | 「送信したこと」と「ユーザーに伝わったこと」を明確に区別し、確認が取れてから完了状態にします。 |
| **個人データの完全削除** | 消去条件の先行永続化＋各受入箇所でのロックフリーな条件照合＋削除操作ごとの完了集約の直列化。全ドメインを長時間ロックすることはしない | 新規保存の禁止と実行中処理の安全な停止を両立させるためです。完了の集約部分のみを直列化すれば十分です。 |
| **バックアップと復元** | ステージング領域での並行検証＋マスターポインタ切り替えの瞬間のみごく短い排他制御。読み取り処理の全面停止はしない | マスター切り替えの瞬間だけアトミック性を守ればよく、全期間にわたるグローバルロックは過剰であるため採用しません。 |
| **ファイルシステムとDBの連携** | 保存先ディレクトリの永続化 → 一時ファイル書き込み → ファイル同期 → 公開保護の確立 → 未使用名へリネーム → ディレクトリの永続化 → DBポインタ登録 → 公開保護解除 → 不要ファイル掃除 | DBとファイルシステムは別の永続化領域であるため、正しい順序制御とクリーンアップ処理の同期によって整合性を保ちます。 |
| **処理のキャンセル** | キャンセル通知シグナル（Notify や cancellation token）＋協調的停止＋確定処理の禁止（新規 work の admission と現在 Task への採用・lifecycle 前進に限定。already-started activity の事実記録は妨げない）という多層防御。非同期タスクの drop を停止完了とみなさない | 外部サービスへの作用は巻き戻しができないため、キャンセルの伝達、実際の停止完了、結果の隔離を明確に分離します。 |

> [!IMPORTANT]
> **本書で採用しない複雑な仕組み**
> 分散合意プロトコル（Raft/Paxos）、分散リース、全域グローバルロック、巨大なグローバルトランザクション、すべてをひとつのアクターに集約する設計、論理時計（Lamport/Vector clock）などは採用しません。これらはホストPCが一元管理を行う本システムのアーキテクチャでは不要であり、システムの複雑性を無駄に増大させるためです。

## 7. タスク / 委任 / 追加指示（Task / delegation / steering）

### 7.1 前提と直列化

- **タスクの作成**: タスク本体、初期リビジョン、初期コンテキスト、および確定した場合のワークスペース紐付けをひとつのトランザクションで不可分に永続化（AU2）し、コミットが完了するまでは委任先やエージェントから見えないようにします（durable-before-visible）。
- **追加指示（steering）**: ユーザーからの新しい指示は、タスクの最新リビジョンとコンテキストを安全に前進させるアトミックな更新（AU4）として記録し、過去のリビジョンも履歴として残します。同じ不分区間で現在の `task.progress` が非 terminal であることを必須とし、terminal は `TaskTerminal` として何も書き込みません。過去の指示で作られた結果を新しい目標に自動適用してはいけません。
- **委任エージェントの作成**: 親タスクの期待リビジョンと一致し、かつ現在の `task.progress` が非 terminal であることをアトミックに確認（AU3）した上で作成し、`Started → InProgress` を同じトランザクションで進めます（terminal は `TaskTerminal` で書き込みなし）。同じタスクの同じリビジョンに対する複数の委任は許可し、単一委任の制約を課しません。再委任やリトライは、既存の委任 identity を再利用せず、新しい委任 identity で開始します（委任 identity の replay・重複排除は、それを必要とする producer を持つスライスで追加します）。既に final result で seal された delegation D1 を再利用せず、Task が non-terminal のままなら新しい delegation D2 を AU3 で作成して work を継続できます（D2 は新しい execution lifetime であり、D1 の Action 試行集合を引き継ぎません）。
- **タスクの中断（Cancel）**: 中断要求は個体調整（会話）または第一者管理経路から作業担当へ直接届き、本体LLM・Task Agent の終了を待たずに、`SD-Task` の短い `Immediate` トランザクションで会話起点なら body-free intent PK から Owner role・Companion・message ID・accepted sequence・対象 Task・直接的中断意図を照合し、元 intent / Task に結ぶ後続の型付き `ExplicitSupersession` がある場合だけ `Superseded`（書き込みなし）、曖昧なら `NeedsClarification`（受付保留）とし、管理経路なら認証済み操作 source / 対象 Task を照合して会話の現在性を要求せず、現在の `task.progress` が非 terminal のときだけ `cancelled` へ CAS します（AU16。受理 = commit）。revision・purpose の前提を持たないため、同時 steering と競合しても revision stale にはならず、会話 source の帰属と同じ Task の中断を撤回・置換する後続指示がない限り勝った順に durable に残ります。受理は実行中の provider I/O・外部作用の停止完了を意味せず、停止はベストエフォートです。cancel が禁じるのは新規 work の admission（新規委任・steering・推論 claim・Action 開始）と現在 Task への採用・lifecycle 前進（AU15b の `adopted_revision` 刻印と terminal CAS）だけであり、already-started activity の事実記録（AU15a の到着 record と seal、AU15b の検証済み `task_result_attempt` 相関、Action certainty の CAS、利用量・未伝達登録などの事実記録）は妨げません。開始済みの inference attempt・`data_use` 相関・`action_attempt` を削除・書き換えせず、`Unknown` を成功・失敗・未実行へ変えません。cancel 専用の停止フラグ・停止行・gate 条件は持たず、既存の非 terminal gate が以後の新規委任・steering・推論 claim・Action 開始・結果採用を拒否します。`Cancelled` は吸収的であり、中断解除・progress の巻き戻し・同じ Task の delegation の再利用・再出力を認めません。新たな Owner 意図がある場合、通常 Task は別 Task・新 delegation / agent、Backup 専用 Task は認証済み Owner Run now または将来の別の予定回に基づく別 Task・point の agentless producer（AU3 なし）とし、現在の Permission・設定・Stop・消去・復元と旧 `Unknown` の二重実行リスクを再評価します。Run now は元の missed occurrence の key / status / cursor を変更しません。手動管理 Direct は Task を要しません。
- **直列化の範囲**: 同一の `TaskId` に対する追加指示、委任の受付、中断の受付、完了の確定、結果の到着 record・採用判定は、`SD-Task` ドメインにより順番に直列化します。異なる `TaskId` 同士は完全に並行して実行できます。直列化の実現メカニズム（per-Task mailbox / per-Task Mutex / 短い即時トランザクション内の比較照合）は Design Freedom です。委任作成の現在性は、単一の書き込み接続が担う短い `Immediate` トランザクション内で、期待リビジョン・現在のタスクリビジョン・非 terminal な progress を比較照合（CAS）することで保証します。結果の到着 record は result identity の PK（再 arrival の 1 回だけの記録）と 1 delegation につき final result 最大 1 つ（UNIQUE(delegation_id) 相当）で本文と execution seal を保護し、採用・完了確定の現在性は、delegation 行から解決する依拠リビジョンと現在 `task.revision` / purpose identity の一致、非 terminal な `task.progress` からの terminal への CAS、delegation（execution lifetime）から列挙した `action_attempt` の authoritative set と claim の完全一致、`Unknown` の阻止と無作用失敗の検証済み非依拠判断の読み取り、および Task-wide completion barrier（同じ `TaskId` に属する全 revision / 全 delegation の Action 試行に `Unknown` が無いこと）の比較を同じ不分区間で行うことで保証します（リビジョン CAS がすべてのメンバーの直列化を代替するわけではありません）。per-Task mailbox / Mutex は、同一プロセス内で同じ Task のタスクリビジョンまたは現在値を更新する producer が増えるスライスで、必要になった時点で追加します。

### 7.2 競合制御のルール

AU2 の新 Task と AU3 の新 delegation も、担当 Companion の canonical `Running` と削除開始 hold 不在を Stop の `Stopped` commit / hold 確定と同じ SQLite master の短い比較・commit 点で確認します。事前 read や既存 Task の非 terminal を代用しません。`Stopped / Deleted / deletion hold → AU2 / AU3` なら通常の AU2 / AU3 は `CompanionUnavailable` で Task / delegation の新規行は 0、`AU2 / AU3 → Stop` なら確定した Task / delegation を保持して Cancel と停止を best-effort で試みます。AU18 に含む AU2 もこの比較を共有しますが、Stop 中の予定回は `Missed`・Task なしとします。開始済み attempt、結果、`Unknown` を Stop で消しません。

AU14 claim と provider first-byte の間では Cancel による terminal、steering の revision 前進、final result の execution seal、推論同意 / assignment の撤回も最終 dispatch と競合します。Task Agent は同じ master の短い送信開始境界で Task の非 terminal progress / current revision / 未 seal、現在の Capability 同意・割当・認証情報、Companion lifecycle / hold と現在の消去条件を、費用現在性・UTC window・全適用 cap とともに再比較します。変更の commit が先なら旧 claim からの送信は 0 byte で止め、first-byte が先なら送信済みの事実を保持し、以後の送信は改めて gate を通します。送信開始が区間内に確定できなければ解放して再照合します。0 byte と元 ticket の送信不能を証明できたときだけ未送信と予約 `Released` を同じ commit で確定し、証明不能なら元 claim / `data_use` と予約・usage `Unknown` を保持します。provider 応答待ちに DB transaction / Task-wide lock を保持しません。

AU14 inference claim と Companion 付き AU5 Action start は Task progress / execution seal の該当条件に加え、担当 Companion の durable `Running` と削除開始の新規活動禁止 hold 不在を Stop の `Stopped` commit / hold 確定と同じ SQLite master の短い `Immediate` 比較点で判定します。Stop / hold 先勝ちなら独立の `CompanionUnavailable`（AU14 は provider 0 byte、AU5 は作用 0、双方 attempt 0）。claim / start 先勝ちなら開始済み試行・利用量・`Unknown` を保持して停止は best-effort です。AU14 claim 後 provider first-byte、Companion 付き AU5 後実作用開始の最終 gate も同じ状態を削除条件とともに再照合し、Stop が先なら開始せず、既に開始したものは遡及取消ししません。比較と first-byte / 実作用開始の間に Stop が割り込めない短い境界とし、開始をその区間内で確定できなければ解放して再照合します。外部 I/O 完了を待って DB lock を保持しません。Taskless Direct の Companion 付き経路も同じ gate を使います。管理起点の `Direct { companion: None }` は Stop に依存せず、AU5 と最終 gate で管理操作・system operation・許可対象と削除 / 復元保留を比較します。

例えば Stop が `Stopped` を commit した直後、別 owner の Task cancel が未確定で Task がまだ `InProgress` でも、AU14 / AU5 は Companion gate で拒否します。逆に AU14 / AU5 の commit が先なら Stop はその既開始 fact を消さず、最終送信 / 作用 gate が Stop を先に見た場合だけ未開始 I/O を阻止します。Direct Action でも Task progress が存在しないことを許可証にしません。

- **指示の競合（steering vs steering）**: 2つの追加指示が同時に届いた場合、`SD-Task` の順序で直列化され、先に確定した指示が最新リビジョンとなります。後から届いた指示は、その新しいリビジョンに対するさらなる変更として評価します。指示を勝手に混ぜ合わせたり上書きしたりしてはいけません。会話のログ記録と、タスクに反映された内容、および保留中の指示は明確に区別し、反映できなかった場合は理由と選択肢を提示します。
- **委任の競合（delegation vs steering）**: 委任関係は作成時点のタスクリビジョンとスコープのコピーを保持します（`DelegationRef`）。追加指示によってタスクのリビジョンが進んだ後に、古いリビジョンを前提とした委任作成要求が届いた場合は受理せず、最新指示のもとで再評価させます。実行中の委任と追加指示が交差した場合、親側は古い委任の停止を試み、古い委任の結果を新しい目的に勝手に流用しません。
- **Task Agent の推論開始 vs steering（inference start vs steering）**: Task Agent の推論は、推論試行の確定（attempt claim）と同一の短い `Immediate` トランザクションで、(1) 前提の委任が `delegation` 行に存在すること、(2) その行の `(task_id, task_revision)` が依拠 `TaskRef` 前提と一致すること、(3) 現在の `task` 行のリビジョンが依拠リビジョンと一致すること、(4) 現在の `task.progress` が非 terminal であること、(5) その delegation が seal 済みでないこと（その delegation の `task_result` 行が存在しないこと）、の 5 条件に加えて、(6) 論理入力の canonical source 相関（`data_use`。採用目的 entry と全採用指示 entry の `origin.source`。本文・hash を含まない）が現在の durable な消去条件（`ErasureConditionRef` 相当）に覆われていないこと、を照合してから開始します。委任行・タスク行の欠如や (3)(4)(5) の不一致は `TaskPremiseStale`、同意・認証情報の不一致は `Stale`、(2) の不一致や部分的・不整合な行は技術的エラー（fail closed）として区別し、新規推論を provider へ一切送信しません。消去条件の coverage がある場合は `DataUseHeld`（data-use hold）として区別し、`TaskPremiseStale` / `Stale` へ丸めずに provider I/O を開始しません。推論側は Task lifecycle の値型を import せず (4)(5) を `TaskPremiseStale` として拒否し、作業側の再読込が progress の terminal を検出した場合は `StaleTaskRevision` ではなく `TaskTerminal { task, progress }`、`load_delegation_result(delegation)` で delegation の final result（seal）を検出した場合は `TaskAgentTurnOutcome::ExecutionSealed { delegation }` として説明します（terminal と execution seal は別概念であり、Task が `InProgress` のままでも delegation は seal 済みになり得ます）。AU14 と AU15a（結果到着 record と seal）は同一 SQLite master 上の短い `Immediate` トランザクションで直列化されるため、`AU14 commits → AU15a seals` ならその推論試行は開始済みの durable fact として残り、`AU15a seals → AU14 arrives` なら AU14 は送信前に拒否し provider I/O を開始しません。確定済みの試行は、その後の追加指示によって開始を取り消されません（結果の採用判定は別の不分区間で行い、遅延結果は元リビジョンへの記録に留めます）。採用指示本文の History bounded read（`message_id` PK の単一メッセージ）は claim トランザクションの外で行い、その間 Task-wide mutex や長い SQLite transaction を保持しません。context と本文を読んだ後に steering が勝った場合は、claim が依拠 TaskRef の不一致を拒否するため、古い context から組み立てた prompt は provider へ送られません。claim と消去条件の durable 化は同一 SQLite master 上の短い `Immediate` トランザクションで直列化されるため、Targeted Deletion との間にも次の 2 順序しかありません: 「消去条件が durable → claim が data_use の coverage を検出 → `DataUseHeld` で拒否、provider へ 0 バイト」、または「claim commit → attempt 行（data_use 相関つき）が already-started use として durable → provider I/O 開始 → その後の deletion 開始は attempt を削除参加として列挙し、停止・縮小・遅延結果の再保存防止の対象にする」。claim 前に materialize 済みの本文・scrub 済み prompt はこの照合を通らなければ送信されず、拒否時は cache・retry へ残さず破棄します（claim 前に deletion status を再読込する方式では、再読込と送信の間に同じ race が残るため採用しません）。provider I/O は試行確定後にロックなしで実行し、推論試行自体の自動再実行は行いません。
- **会話入力の受理 vs AU16（typed source の比較点）**: 個体調整が認証済み Owner 入力を受けると、会話文脈と現在 Task 候補で直接的中断意図と対象を確定し、明確な「止めて」は本体 LLM / Task Agent を待たず、入力と body-free intent を同じ master に不可分に記録して AU16 へ渡します。対象未確定なら確認待ちにして intent を発行せず、確認入力の受理時に確定します。後続入力の明示撤回・置換または曖昧な撤回は元 intent / Task に相関した `ExplicitSupersession` / `PendingClarification` として当該入力の受理と不可分に記録し、確認結果も元 pending に結びます。AU16 は intent PK とその索引付き後続範囲を progress CAS と同じ短い `Immediate` transaction で読むため、後続の明示撤回が先なら `Superseded`、未解決の確認待ちが先なら `NeedsClarification`、無関係な入力なら cancel を受理します。AU16 が先なら terminal は吸収的で後続入力は巻き戻しません。重複・Host crash / replay・History retention / erasure 後にも body-free 帰属を読み直し、消去済み本文から再推論しません。provider output / 未認証第三者は fact を発行できず、異なる Task / Companion への付け替えは技術的エラーです。

- **キャンセルの競合（cancel vs 各 commit）**: 中断要求は `SD-Task` の短い `Immediate` トランザクションで会話起点なら body-free intent PK と元 intent / Task に結ぶ後続の型付き撤回・確認待ち、管理経路なら認証済み操作 source / 対象 Task を照合し、どちらの source でも現在の `task.progress` を比較し、非 terminal のときだけ `cancelled` へ CAS して即座に受理します（AU16。同じ Task の中断を明示撤回・置換した後続 Owner 指示が先なら `Superseded` で書き込み・停止通知なし。曖昧なら `NeedsClarification` で受付保留。第一者管理経路には会話 currentness を課しません。revision・purpose の前提はなく、同時 steering の前進を取り消しません）。受理と実際の停止完了は区別し、停止できなかった処理や成否不明な外部作用は事実として記録して報告します。この直列化で cancel が禁じるのは新規 work の admission と現在 Task への採用・lifecycle 前進だけであり、already-started activity の事実記録（AU15a の到着 record/seal、敗者となった AU15b の検証済み `task_result_attempt` 相関、Action certainty の CAS、利用量など）は妨げません。受理と各 commit は同一 SQLite master 上の短い `Immediate` トランザクションで直列化されるため、考えられるのは次の順序だけです: (1) AU3 委任作成が先 → delegation 行は残るが、その execution からの AU14 claim / AU5 開始は terminal gate で拒否される（AU14 は claim 内で `TaskPremiseStale` として provider に 0 バイト、作業側で `TaskTerminal { progress: Cancelled }` へ写像。AU5 は `TaskTerminal`）。中断が先 → AU3 は `TaskTerminal { progress: Cancelled }` で delegation を作成しない。(2) AU4 steering が先 → revision 前進は durable に残り、会話 source の帰属が成立し同じ Task の中断への明示撤回・置換がなければその上で中断が記録される（撤回・置換が先なら `Superseded`）。中断が先 → AU4 は `TaskTerminal` で前進しない。(3) AU14 claim が先 → attempt 行と `data_use` は送信開始の証拠ではなく相関として残る。Cancel が provider first-byte より先なら最終 dispatch gate で旧 ticket の送信を拒否し、0 byte と以後の送信不能を証明できた場合だけ `NotSent` と予約 `Released` を不可分に確定する。証明不能なら元の予約・利用量 `Unknown` を保持し、first-byte が先なら送信事実を残して abort をベストエフォートで試みる。中断が先 → claim は `TaskPremiseStale`（作業側で `TaskTerminal`）で provider に 0 バイト。(4) AU5 が先 → `action_attempt` は `Unknown` のまま残り、その確定度は ene-action owner の客観的証拠によってのみ更新される（cancel は書き換えない）。中断が先 → AU5 は `TaskTerminal` で挿入・実行ともに行わない。(5) AU15a 到着 record/seal は progress を判定しないためどちらの順序でも成立するが、消去条件 / provenance を照合して本文は held になり得る。AU15b 完了確定が先 → `Completed` のままで、中断は `TaskTerminal { progress: Completed }` を返す。中断が先 → AU15b は `task.progress` の terminal を検出して attempt 相関の記録に留め、`adopted_revision` を刻まず `RecordedToOriginalOnly` を返す。キャンセル後に遅れて届いた final result は元リビジョンの履歴に留め、現在 Task の完了成果として受け入れてはいけません。再要求は会話 source の帰属が成立し同じ Task の中断への明示撤回・置換がなければ `AlreadyCancelled` として冪等に返し、後続の明示撤回・置換があれば `Superseded` とし、受理 record を重複させません。`Cancelled` は吸収的であり、中断解除・progress の巻き戻し・同じ Task の delegation の再利用で再開せず、改めて実行する通常 Task は新 Task・新 delegation、Backup 専用 Task は新たな Owner Run now または将来の別の予定回から新 Task・point を agentless に実行し、AU3 を行いません。
- **遅れて届いたエージェントの結果（delayed Agent result）**: final result の到着はまず result identity・安全な本文または body-free held の 1 回だけの durable record と、同じ不分区間での delegation の execution seal とし（採用・完了判定より先）、その後の採用判定で帰属を delegation 行 → 依拠タスクリビジョン → 現在のタスクの順に辿って検証します（CI §6.3）。採用判定は 1 つの短い `Immediate` トランザクションで、delegation（execution lifetime）から列挙した `action_attempt` の authoritative set と claim の完全一致（欠如・追加・重複は技術的エラー。membership は seal 時点で固定）、現在 `task.revision` が依拠リビジョンと一致するか、purpose identity が依拠リビジョンの `task_revision` snapshot と一致するか（本文の文字列一致は使いません）、`task.progress` が非 terminal か（`Cancelled` は terminal であり、cancel の producer 追加後も cancel 専用の条件を足さない）を照合します。現在 revision と一致して完了へ進む場合は、同じ不分区間で Task-wide completion barrier（同じ `TaskId` に属する全 revision / 全 delegation の Action 試行に `Unknown` が無いこと）も検証し、1 件でも `Unknown` が残れば result-local の attempt 相関のみを記録して `WithheldByEffectFacts` を返します。一致しない場合は attempt 相関のみを記録し、現在のタスクや会話へ反映せず `RecordedToOriginalOnly` を返します。
- **タスク完了の競合**: タスクの完了確定は、`SD-Task` の短いトランザクション内で「result 行が到着 record 済みか」「delegation（execution lifetime）から列挙した authoritative set が claim と完全一致するか（membership は seal 時点で固定）」「現在のリビジョンが依拠リビジョンと一致するか」「authoritative set の `Unknown` が無く、失敗は `RefusedBeforeEffect` の確認済み無作用に対する Task owner の代替達成・残作業・客観的根拠の検証と非依拠記録が成立するか（部分的作用・不足は保留。空集合は durable に空の場合のみ）」「同じ `TaskId` に属する全 revision / 全 delegation の Action 試行に `Unknown` が無いか（Task-wide completion barrier。result-local set の外も対象）」「`task.progress` が非 terminal か」を確認して行います。AU5 Action 開始と AU15b 完了確定は同一 SQLite master 上の短い `Immediate` トランザクションで直列化されるため、`AU5 commits A2 → AU15b` なら barrier が A2 を検出して完了を保留し、`AU15b Completed → AU5` なら AU5 が terminal gate で `TaskTerminal` として拒否され、A2 の行も外部作用も発生しません（この 2 順序以外の隙間はありません）。確定度更新と AU15b も同じ master で直列化され、`Unknown → Confirmed*` が先なら barrier は clear、AU15b が先で `Unknown` を見れば `WithheldByEffectFacts` となり、その後 A2 が settlement すると同じ result の再評価で完了が成立し得ます（Task owner は確定度を書き換えません）。複数の完了報告（異なる delegation の異なる result）が交差した場合は、`Immediate` トランザクションと progress の CAS により先に確定した方のみが完了し、後から届いた報告は attempt 相関のみを記録して `RecordedToOriginalOnly` とします（二重完了にしません）。完了後に遅れて届いた古い result は本文の消去 gate を経て履歴へ帰属させ、現在 Task を変更しません。同じ delegation への 2 つ目の final result は AU15a が受理せず fail closed です。
- **再試行と再起動（retry / restart）**: 通常 Task のリトライや再委任は、その時点で依拠できる現在のタスクリビジョンを前提として、新しい委任 identity・新しい試行（Attempt）として開始します（同じリビジョンへの再委任・リトライも既存の委任 identity を再利用しません）。`Cancelled` の Task は中断解除・progress の巻き戻し・同じ Task の delegation の再利用・再出力で再開しません。新たな Owner 意図に基づく通常 Task は新 Task・delegation / agent、Backup 専用 Task は認証済み Owner Run now または将来の別の予定回による新 Task・point の agentless producer（AU3 なし）とし、現在の Permission・設定・Stop・消去・復元と旧 `Unknown` の二重実行リスクを再評価します。Run now は元の missed occurrence の key / status / cursor を変更せず、手動管理 Direct は Task を作りません。ホスト再起動後は、保存済みの進捗、確認済みの外部作用、成否不明な状態をユーザーに提示して再開指示を待ち、エージェントを無条件で勝手に自動再開してはいけません。再起動で失われるのはローカルの Future・cancellation token・停止 handle だけであり、開始済みの provider 処理・外部作用が停止したとは推定せず、durable な attempt と `Unknown`、遅延結果の帰属を維持します。再起動で中断された delegated execution は、同じ delegation の続きとして自動再開せず、最初に commit した durable attempt（AU14 claim または AU5 開始）を start marker として、開始済み unsealed execution の再 run を拒否します（`ExecutionAlreadyStarted`）。同一プロセス内で同一 delegation の二重 run も per-delegation registration が開始前に拒否し（`ExecutionAlreadyRunning`）、ローカル registry の非登録・消失を停止や開始可否の証拠にしません。通常 Task の継続は常に新しい delegation です。

### 7.3 処理フローの例（追加指示 vs 結果到着）

```text
-- 追加指示の適用 (SD-Task の短いトランザクション)
BEGIN IMMEDIATE;
  IF task が存在しない THEN 
    ROLLBACK;
    RETURN MissingTask { task }; -- タスクが存在しないため書き込みなしで終了
  END IF;
  IF cur.progress が terminal THEN
    ROLLBACK;
    RETURN TaskTerminal { task, progress: cur.progress }; -- 吸収的な終端。revision stale と区別し、書き込みなしで終了
  END IF;
  IF cur != expected_revision THEN 
    ROLLBACK;
    RETURN StaleExpected { current: (task, cur) }; -- 前提リビジョンが食い違っているため書き込みなしで終了
  END IF;
  -- next = cur + 1。次のリビジョンを永続化データとして確定できない場合は RETURN RevisionExhausted { task }
  INSERT INTO task_revision(task_id, next, 新目的または直前値・担当);
  UPDATE task SET revision = next, 現在 purpose・目的本文 = 新目的または直前値 WHERE task_id = ?;
  INSERT INTO task_context_entry(entry = premise.adopted_purpose_entry, reference = (task, next), 採用目的 identity, 由来, 取得時点);
  -- None（目的変更なし）の由来・取得時点は、同一トランザクション内で現在リビジョンの採用目的項目から引き継ぐ
  -- 追加コンテキスト種別は前提構造体が列挙する（H-A: premise.adopted_instruction があれば採用指示項目を同一トランザクション内で
  -- 挿入する。識別子は前提条件が保持し、リポジトリは reference のみを刻む。項目は採用リビジョンで 1 度だけ書き込み、
  -- 後の前進では再記録しない）。安全保留スライスにおける保留照合も同一トランザクションに加わる
COMMIT;

-- タスク中断の受付 (SD-Task の短い Immediate トランザクション。個体調整（会話）または
-- 第一者管理経路から直接。LLM・Task Agent の終了を待たない。revision・purpose は比較しない。受理 = この commit のみ)
BEGIN IMMEDIATE;
  cur = SELECT progress FROM task WHERE task_id = command.task;
  IF cur が存在しない THEN
    ROLLBACK; RETURN MissingTask { task };
  END IF;
    IF command.source = OwnerConversation { intent } THEN
      -- 会話担当が Owner 入力受理時に同じ master に記録した body-free intent を PK で読む。
      -- message ID・accepted sequence・Owner role・Companion・Task・直接的中断意図を照合。
      -- 欠如・不整合・別 Task への付け替えは技術的エラー。本文や LLM 出力を推測で読まない。
      IF intent の帰属が command.task と不一致 THEN
        ROLLBACK; RETURN 技術的エラー;
      END IF;
      IF 元 intent / Task に結ぶ後続 ExplicitSupersession が索引付き EXISTS で見つかる THEN
        ROLLBACK; RETURN Superseded;
      END IF;
      IF 元 intent / Task に結ぶ未解決 PendingClarification が索引付き EXISTS で見つかる THEN
        ROLLBACK; RETURN NeedsClarification;
      END IF;
   ELSE IF command.source = FirstPartyManagement { source } THEN
     -- 認証済み管理操作 source と対象 Task・Owner の直接指定を同じ master で照合する。
     -- 不明瞭な対象・意図は確認まで保留。会話の最新入力・撤回比較は課さない。
     IF source の認証・Owner・対象 Task の対応が不整合 THEN
       ROLLBACK; RETURN 技術的エラー;
     END IF;
     IF 対象または直接的中断意図が曖昧 THEN
       ROLLBACK; RETURN NeedsClarification;
     END IF;
   END IF;
  IF cur = 'cancelled' THEN
    ROLLBACK; RETURN AlreadyCancelled;  -- 冪等。受理 record は 1 回だけ
  END IF;
  IF cur IN ('completed','failed') THEN
    ROLLBACK; RETURN TaskTerminal { task, progress: cur };  -- 終端。中断で巻き戻さない
  END IF;
  UPDATE task SET progress = 'cancelled'
    WHERE task_id = command.task AND progress IN ('started','in_progress');
  -- 同一 master 上の比較と矛盾する更新行数 0 は技術的エラー（開始済み attempt・Unknown は変更しない）
COMMIT;  -- 受理の durable marker。停止完了・外部作用の確定度は判定も待機もしない

-- 結果の到着記録と execution seal (SD-Task の短い Immediate トランザクション。
-- Task Agent が final result を提出した finalization 境界で、orchestrate が最終結果を可視化する前に実行)
-- 現在性・確定度・terminal・完了は判定しない（採用は AU15b）。1 回の inference turn の provider 出力は入れない
BEGIN IMMEDIATE;
  delegation = SELECT task_id, task_revision FROM delegation WHERE delegation_id = ?;
  IF delegation が存在しない THEN
    ROLLBACK; RETURN 技術的エラー;  -- record 前提の対応が無い。採用時のドメイン結果は AU15b が返す
  END IF
  IF 同じ result_id の task_result 行が既に存在 THEN
    -- 本文は再保存しない。既存 disposition / delegation / 依拠リビジョンを維持して冪等、
    -- 不一致なら ROLLBACK; RETURN 技術的エラー（identity 再利用）
  ELSE IF 同じ delegation の task_result 行が既に存在 THEN
    -- 1 delegation につき final result は最大 1 つ。2 つ目の final result は受理せず
    ROLLBACK; RETURN 技術的エラー;  -- fail closed（domain outcome にしない）
  ELSE
    -- task_id / task_revision は delegation 行から写す（arrival は再掲しない）
    -- 同じ master 内で active source / mechanical condition と completed provenance cut を照合する。
    -- covered / 判定不能 / 旧由来なら disposition = erasure_held、body = NULL。
    INSERT INTO task_result(result_id, task_id, task_revision, delegation_id, arrival_disposition, body, recorded_at)
      VALUES (result_id, delegation.task_id, delegation.task_revision, delegation_id, disposition, safe_body_or_null, now);
    -- この行の commit がそのまま delegation の execution seal になる（seal 専用の列・行を置かない）
  END IF
 COMMIT;  -- 安全な本文または body-free 到着・identity・seal が durable。adopted_revision は NULL

-- 結果の採用・完了確定 (SD-Task の短い Immediate トランザクション。到着 record/seal の後)
-- 依拠リビジョンは delegation 行から、目的は依拠リビジョンの task_revision snapshot から解決し、
-- purpose text の文字列一致では照合しない
BEGIN IMMEDIATE;
   result_row = SELECT task_id, task_revision, delegation_id, arrival_disposition, adopted_revision
    FROM task_result WHERE result_id = ?;
  IF result_row が存在しない THEN
    ROLLBACK; RETURN MissingResult { result };  -- 識別子不在は書き込みなしの正常系
  END IF
  delegation = SELECT task_id, task_revision FROM delegation WHERE delegation_id = result_row.delegation_id;
  IF delegation が存在しない THEN
    ROLLBACK; RETURN MissingDelegation { delegation: result_row.delegation_id };
  END IF
  IF (delegation.task_id, delegation.task_revision) != (result_row.task_id, result_row.task_revision) THEN
    ROLLBACK; RETURN 技術的エラー;  -- 到着 record 後の対応破損。stale へ丸めない
  END IF
  -- delegation（execution lifetime）から authoritative set（この delegated execution が durable start した
  -- Action 試行の完全な集合。membership は AU15a の seal 時点で固定）を列挙する
  attempts = SELECT attempt_id, delegation_id, task_id, task_revision, certainty
    FROM action_attempt WHERE delegation_id = result_row.delegation_id;
  IF attempts の (delegation, task, task_revision) が result_row と不一致
     OR claim.attempt_refs と集合として不一致（欠如・追加・重複） THEN
    ROLLBACK; RETURN 技術的エラー;  -- 不整合な単位。stale / Withheld へ丸めない
  END IF
  cur = SELECT revision, purpose_adopted_revision, progress FROM task WHERE task_id = result_row.task_id;
  IF cur が存在しない THEN
    ROLLBACK; RETURN MissingTask { task: result_row.task_id };
  END IF
  IF result_row.arrival_disposition NOT IN ('recorded', 'erasure_held', 'body_erased', 'legacy_body_unverified') THEN
    ROLLBACK; RETURN 技術的エラー;  -- 未知の disposition を採用しない
  END IF
  IF result_row.adopted_revision IS NOT NULL THEN
    -- この result 自身が過去に採用を確定済み。二度目の terminal transition は行わず同じ判定を返す
    IF result_row.adopted_revision != result_row.task_revision
       OR result_row.arrival_disposition = 'erasure_held' THEN
      ROLLBACK; RETURN 技術的エラー;  -- 採用リビジョンの不整合、または到着時保留の採用は破損
    END IF
    COMMIT; RETURN AdoptedAsCompletion((task, result_row.adopted_revision)); -- body_erased / legacy_body_unverified も過去の採用事実のみ（本文なし）
  END IF
  IF result_row.arrival_disposition IN ('erasure_held', 'body_erased', 'legacy_body_unverified') THEN
    INSERT task_result_attempt 相関（attempts）;  -- 検証済み result-local facts のみ、本文なし。1 回だけ。採用・progress は不変
    COMMIT; RETURN result_row.arrival_disposition に対応する
      ErasureHeld { result } / BodyErased { result } / LegacyBodyUnverified { result };
  END IF
  IF cur.revision != result_row.task_revision OR cur.progress が terminal（'completed' / 'failed' / 'cancelled'）THEN
    INSERT task_result_attempt 相関（attempts）;  -- 1 回だけ
    COMMIT; RETURN RecordedToOriginalOnly;
  END IF
  -- Task-wide completion barrier（同じ TaskId の全 revision / 全 delegation。steering 前の古い revision の
  -- started Action を含む）。入力は action_attempt の durable facts だけを使い、timestamps・delegation liveness・
  -- ephemeral alive flag・result body・caller の attempt_refs は使わない。不整合な行は技術的エラーとして fail closed
  unknown_attempts = SELECT attempt_id FROM action_attempt
    WHERE task_id = result_row.task_id AND certainty = 'unknown';
  -- 失敗試行を非依拠とする場合、Task owner は現在目的の代替達成、残作業なし、客観的な検証記録の参照を確認する。
  -- assessment の非依拠対象は attempts 内の confirmed_failure + refused_before_effect に限る。
  -- 検証記録の存在・対応・現在性を再照合し、LLM の新たな自己申告だけでは成立させない。
  -- partial effect / 根拠不足は非依拠にできず、Action owner の certainty / grounds は変更しない。
  blockers = 集合（attempts の unknown / 非依拠が検証できない failure）∪ 集合（unknown_attempts）; -- 重複は 1 回
  IF blockers が空でない THEN
    INSERT task_result_attempt 相関（attempts）;  -- result-local の検証済み集合のみ。barrier の試行は刻印しない
    COMMIT; RETURN WithheldByEffectFacts { attempts = blockers }; -- 非依拠に必要な根拠不足は対象の失敗試行を返す
  END IF
  INSERT task_result_attempt 相関（attempts）と非依拠判断・理由・根拠参照; -- authoritative set 全件。1 回だけ
  RECORD Task owner の目的達成の根拠参照・残作業なしの判断; -- 採用と不可分。再評価時も上書きしない
  UPDATE task_result SET adopted_revision = cur.revision WHERE result_id = ? AND adopted_revision IS NULL;
  UPDATE task SET progress = 'completed'
    WHERE task_id = ? AND progress NOT IN ('completed','failed','cancelled');  -- terminal への CAS（barrier 判定と同一トランザクション）
  COMMIT; RETURN AdoptedAsCompletion((task, cur.revision));
```

採用判定では、`purpose` を identity / adopted_revision の対応で照合し、目的本文の文字列一致を使いません。Action の確定度は ene-action owner の事実のまま読み取るだけで、Task owner は書き換えません。`Unknown` は新しい客観的証拠が確定するまで保持し、seal 後に証拠が付いて `ConfirmedSuccess` へ進んだ場合は同じ result の再評価で完了が成立し得ます（membership は seal 時点のまま）。attempt 集合は delegation（execution lifetime）から durable に列挙した authoritative set だけであり、caller の `attempt_refs` はこの集合との完全一致にのみ使います（単独で集合を狭めたり広げたりできません）。`recorded` 到着後に削除参加が未採用本文を `body_erased` に変えた場合や V43 → V44 で未採用本文が `legacy_body_unverified` になった場合、同じ master の `Immediate` で直列化された AU15b は検証済み result-local 相関だけを残して各固有 outcome で止まり、barrier が空でも採用しません。既採用の本文消去・旧由来は `adopted_revision` の過去事実だけを返し、`erasure_held` は採用されません。確認済み無作用失敗への非依拠判断と根拠参照は、全試行の相関・結果採用・Completed CAS と同じ不分区間で記録し、保留時に採用済み判断を刻印しません。Task-wide completion barrier は result-local authoritative set とは別の gate であり、同じ `TaskId` に属する全 revision / 全 delegation の試行を `action_attempt` から `task_id` と `certainty` で読み、`Unknown` が 1 件も無いことを Completed CAS と同じ不分区間で要求します（timestamps・delegation liveness・ephemeral alive flag は使いません）。cross-delegation / 旧 revision の `ConfirmedSuccess` / `ConfirmedFailure` は barrier だけを理由に完了を block せず、in-flight の inference attempt と unsealed delegation の存在も barrier に含めません。barrier で見つけた試行は `task_result_attempt` に刻印しません。到着 record と seal は採用判定より先に確定するため、採用判定の成否や途中クラッシュで安全な本文と body-free held の区別を失いません。同じ result identity の retry は本文行と attempt 相関行を増やさず、二重の terminal transition を行いません。同じ delegation の異なる result identity は 2 つ目の final result として受理されません。

### 7.4 resume は Task 種別ごとに残作業を受理する

AU17 は、IB H-A.1 の条件比較と対応する未伝達登録を 1 つの `Immediate` transaction で確定します。通常 Task は新 revision・採用指示 entry・新 delegation / agent・Task 現在値を不可分に作ります。Backup 専用 Task は保存済み種別・Started occurrence / Run now source・目的・現在の `TaskRef = (T, r)` を照合して固定し、認証済み Owner 再開指示の body-free 受付 fact と必要な `Started → InProgress` だけを不可分に確定します。point / Action の依拠 TaskRef を保ち、delegation / agent / 新 revision / 採用指示 entry を作りません。通常 Task の revision 前進は新しい Owner 継続指示と旧 execution を区別するためであり、Backup 専用 Task の同じ point の検証には適用しません。

durable facts だけで実行中かは判定できません。Host は既存 `TaskExecutionRegistry` の短い同期区間に、通常 Task の委任 launch と Backup 専用 Task の agentless 続行の今回限りの予約・登録・解除、resume の「同じ Task に予約/登録なし」の判定を集めます。AU3 / 通常 AU17 は compare/commit した delegation の launch 予約、Backup 専用 AU17 は Task / source の agentless 予約を lock 解放前に登録します。通常 runner は delegation の予約を引き継いで実行し、別の呼出しが DB の delegation ID だけを渡して開始する入口は公開しません。agentless producer も受理済みの予約と現行の AU5 / 完了 gate を通し、DB の Task 一覧から外部作用を自動開始しません。通常 execution の初回 AU14/AU5 の前も予約を確認し、開始済み試行の probe と seal/terminal/revision gate を維持します。attempt が 0 件の古い delegation も、この process に受理された予約がなければ `ExecutionUnavailable` として launch しません。

Backup 専用 Task の AU17 は同じ registry の Task 単位受付と `Immediate` 比較点で通常の AU3 / AU4 / AU5・専用完了・cancel と直列化します。全 revision の `Unknown` があれば `HeldByUnknownEffects`、出力試行 0 件なら同じ point に対する未開始の agentless 作業だけを予約し、既存 `ConfirmedSuccess` なら外部作用なしで保全 owner の同じ point の検証結果を再評価します。point が成功なら専用完了、回復不能な失敗なら外部 Create の成功 fact を保持して `fail_task` の条件を評価します。Action が `ConfirmedFailure` でも同じ Task の新試行は拒否し、別 Task は Owner の明示判断に委ねます。AU5 は出力試行が 1 件でもあれば `UNIQUE(task_id)` で拒否するため、同じ TaskRef の新しい resume 指示でも P2 を出力できません。元の source・point・試行を付け替えず、成功済み試行は同じ TaskRef / source / purpose を検証して専用完了で参照します。agentless 予約を受理した Host だけが続きを開始し、crash 後に予約を復元して作用を自動開始しません。

試行 0 件でも point に回復不能な失敗 marker があれば出力予約には進まず、保全の失敗根拠と外部無作用の証明から `fail_task` の条件を評価します。未準備の point も出力予約の権限にはならず、準備完了と marker 不在を検証してから未開始作業を進めます。point の失敗 marker と AU5 / 作用直前は同じ writer で直列化し、Task の terminal CAS を待つ間も外部出力を閉じます。

AU18 / Run now AU2 の schedule currentness は新 Task の発行時にだけ比較します。開始が先なら、その後の Schedule 変更・停止・削除は既存 Task の AU5 / 最終作用 / AU17 を拒否しません。これらは保存済み source / purpose の正当性と現在の Permission・担当 Companion・`backup_setting`・消去 / 復元を比較します。Run now の元会話入力は AU2 で最新性を証明済みであり、後続の無関係な Owner 入力では失効しません。管理 source identity の失効・偽造、同じ Task の Cancel / steering、個体 Stop / Deletion は独立に直列化して拒否します。

DB を使う同期処理は `spawn_blocking` に移してから registry の短い lock と SQLite transaction を取得します。lock 中の `.await`、provider I/O、外部作用、本文の組立ては禁止です。commit 成功後に通常 runner または Backup 専用 agentless producer の開始が失敗した場合は予約を解除して未実行を表示し、同じ delegation / TaskRef からの外部 work の自動再開始は行いません。resume の受理は durable に残ります。Task registry の不在を根拠にした自動起動スキャンは作りません。

| race / crash | 確定する結果 |
|---|---|
| 同じ r への resume が二つ到着 | 通常 Task は一方だけ r+1・採用指示・新 delegation を作り、もう一方は StalePremise。Backup 専用 Task は同じ r のまま、異なる Owner source の指示なら実行登録中は AlreadyRunning、後の明示指示は別の body-free 受付 fact。同一 source retry は初回 outcome。出力試行は Task ごとに最大 1 件 |
| 通常 Task の resume と AU3 / runner 起動 | AU3 の予約が先なら AlreadyRunning。resume が先なら旧 r の AU3 は stale。Backup 専用 Task への AU3 は常に拒否。登録確認と commit の間へ launch を割り込ませない |
| resume と steering | 通常 Task は revision CAS の先勝ち。Backup 専用 Task も期待 TaskRef の照合で先行 AU4 が許された場合は stale。既存 point の TaskRef を切断する AU4 は拒否する。最新 revision へ要求を自動で付け替えない |
| resume と cancel / confirmed failure / completion | terminal commit が先なら TaskTerminal。通常 resume が先なら旧 failure/result adoption は stale/original-only。Backup 専用 resume は同じ TaskRef のままで、先行完了なら TaskTerminal、先行 resume なら保存済み point の完了 CAS が現在 facts を照合する。cancel は Task 単位で後続 admission を止める |
| 通常 Task の resume と AU15a seal / AU15b adoption | seal が先でも実行登録が残る間は AlreadyRunning。登録解放後に現 revision の採用条件を満たせる結果があれば ResultAvailable。adoption が先なら TaskTerminal。resume が先なら旧 result は元 delegation へ記録のみ。Backup 専用 Task は AU15b に入れず、同じ TaskRef / point の専用完了だけを使う |
| resume と Action start / certainty settlement | Task-wide Unknown の照合を AU5 / settlement と同じ DB transaction 順序に置く。Unknown が残る時点では HeldByUnknownEffects。settlement 先勝ちなら現在 facts で判定。通常 resume 後の旧 revision の新規 AU5 は拒否。Backup 専用は同じ TaskRef のまま一意制約で 2 件目の出力を拒否 |
| AU17 commit 前の crash / commit 後・開始前の crash | 前者は全 rollback。後者は通常 Task の r+1・採用指示・新 delegation、または Backup 専用 Task の同じ r・body-free 受付 fact と通知が残り、予約は失われる。いずれも startup は外部 work を起動しない。Backup 専用 Task の既存成功 facts の完了再評価だけ許す |
| 再開後に旧 execution の結果が届く | 元 identity で AU15a、AU15b は RecordedToOriginalOnly。旧結果で新 TaskRevision を完了せず、Unknown も消さない |
| Backup 専用 Task の resume vs AU5 / settlement / 完了・失敗 | AU5 が先で Unknown なら resume を保留。ConfirmedSuccess なら同じ TaskRef / point の検証結果を再評価し、整合性成功なら完了、回復不能な失敗なら作用成功の事実を保持して `fail_task` の条件を照合する。ConfirmedFailure なら同じ Task の再出力を拒否し、回復不能と検証した場合だけ失敗へ進める。resume が先で未開始なら同じ TaskRef と現行条件で最初の一試行だけ開始可能。完了 / 失敗 / cancel が先なら TaskTerminal。P1 がある Task の同じ r からの P2 は point が異なっても拒否 |

task_report と一覧は現在の Task 行・Backup 専用 Task の source / body-free resume 受付・result・Action facts を read transaction で整合して読みます。表示の「実行登録あり」はメモリ上の補助事実で、次の resume の permission ではありません。resume は必ず上記の比較をやり直します。

Backup 専用 Task の AU2 は、AU18 の Started occurrence または認証済み Owner Run now と指定 schedule の現行条件・担当 Running Companion・Host / 削除 / 復元 hold を同じ `Immediate` 比較点で照合して Task 種別と由来を保存します。Run now は元回の key・status・cursor を更新せず、同じ Owner source の重複は既存 Task を返します。AU3 はこの種別を試行の有無にかかわらず拒否し、AU4 は種別 / source の変更・backup 以外の目的への転換と既存 point の TaskRef を切断する revision 前進を拒否します。AU15b は同じ transaction で通常 Task / source 不在を照合してから採用します。専用 producer は同じ point の成功 marker・唯一の出力試行の `ConfirmedSuccess`・Task-wide Unknown 空を同じ commit で確認します。

通常 Schedule の Run now も AU2 の同じ writer で Owner source の対象 / 現在性、指定版と現行 schedule の版・有効な内容 / 担当 / Workspace、Companion Running と hold を照合し、Task 一式と `normal_schedule_run_now` を不可分に保存します。Schedule 変更・Stop が先なら `StaleSchedule` / `CompanionUnavailable` 等の `TaskCreationOutcome` を書き込みなしで返し、作成が先なら新 Task はその設定 snapshot と保存済み source に基づいて進みます。Backup と通常の Run now は同じ Owner source の二重消費を同一 writer で拒否し、同じ帰属の再送のみ既存 TaskRef を返します。元の missed / unclassified / started 回や cursor には触れず、後続 Schedule 変更によって作成済み Task の委任を失効させません。

Backup 専用 Task の保存先・保護設定が point 準備後・出力前に変わった場合、保全 owner は同じ `Immediate` writer で当該 Task の全 revision に出力試行が 0 件かつ外部未開始を確認し、同じ point の保存先・保護設定と設定 revision を現在の Owner 選択へ CAS で再確定します。AU5 / 作用直前は point と現在の設定を比較し、古い評価 ID・保持実対象を使わず保護方式と出力内容を再検証してから Permission の新評価を要求します。AU5 が先に一試行でも commit したら point の再確定を拒否し、結果 `Unknown` / `ConfirmedFailure` でも同じ Task から別 point へ出力しません。外部未開始が証明できなければ point を変えず保留します。

同じ point の検証済み回復不能失敗を Task owner の `fail_task` に渡すときは、保全の failure fact、Action の試行確定度（`ConfirmedSuccess` でも保全失敗はあり得る）と作用根拠、Task-wide Unknown 空・外部影響の範囲が判明し残作業による達成可能性なし・現在 TaskRef / source / purpose・非 terminal を同じ `Immediate` で比較し、`Failed` CAS と失敗帰属・未伝達を不可分に記録します。試行前の作成不能も外部無作用の証明を要します。`Unknown`、影響範囲が不明な部分作用、再評価可能な設定変更、一時的技術障害を terminal にせず、`Failed` が先なら AU5 / AU17 は `TaskTerminal`、AU5 が先なら新試行の事実を含めて失敗判定を再評価します。

## 8. 外部アクション（認可と実行結果追跡の分離）

外部サービスやツールへの作用（ファイル操作、API呼び出しなど）そのものは、データベースのようにロールバックして取り消せません。そのため、「アクション開始前の厳密な照合」と「開始後の確実な結果追跡」を明確に分離します。

### 8.1 アクション開始前の厳密な照合（短いトランザクション、非同期待機なし）

新しいアクション試行（`action_attempt`）を登録する際、ごく短い `Immediate` トランザクションの中で以下を不可分に照合します：

- association が閉じた `TaskBound` なら `(task_id, 期待されるtask_revision)` が現在のタスクリビジョン、Workspace 用途なら委任・ワークスペース、Backup 専用出力なら保存済みの Started occurrence または認証済み Owner Run now source / point / 出力 scope / 現在設定と一致し、現在の `task.progress` が非 terminal であるか（terminal は `TaskTerminal` として開始しない）。
- Workspace `TaskBound` の委任（execution lifetime）が seal 済みでないか（その delegation の `task_result` 行が存在しない。seal 済みは `ExecutionSealed` として開始せず、Task が InProgress でも拒否する）。`Direct` なら Task / 委任を捏造せず、認証済み Owner instruction source の現在性（会話なら最新 accepted input、第一者管理なら認証済み操作）と許可対象 scope を比較する。`companion = None` は管理起点で `ManualBackupExport` / `CharacterPackageImport` / `CharacterExport` の許可済み system operation と実操作・用途が一致するときのみ認める。
- `TaskBound`（Workspace / Backup 専用出力）/ `Direct { companion: Some(_) }` の担当 Companion が canonical に `Running` で、削除開始の新規活動禁止 hold がないか。Stop の `Stopped` commit と削除 hold の確定はこの同じ SQLite master の比較点と直列化する。管理起点の `Direct { companion: None }` は Stop に依存せず、現在の管理操作と共通の削除・復元保留を照合する。
- 解決された具体的な操作対象と操作種別（読み取り、作成、編集、削除、実行など）が正当か。
- 根拠となる権限評価が、最新のルールリビジョン、ユーザー同意、端末条件、コスト上限、保留状態と合致しているか（過去の許可記録や解決済み経路だけを根拠に開始してはいけません）。
- クライアント端末に依存する処理であれば、端末の滞在世代番号と実際の接続状況が有効か。
- コスト枠の仮押さえ（reservation）が同一トランザクション内で正常に完了しているか（第9節）。
- 個人データ削除や復元の保留状態に抵触していないか。
- `Create / Edit` で Action owner が実際に書く確定済み本文全体、既知の canonical source lineage、操作対象に関わる消去制約が、同じ SQLite master の canonical current erasure condition に覆われていないか。`Read / List` も対象・既知 source の利用を同じ比較点で照合する。本文の機械的 coverage は source coverage の空集合や権限評価では代用せず、照合不能なら `DataUseHeld` とする。
- TaskBound の Workspace `Create / Edit` は中間作業と最終成果物の用途を認可評価・試行・実作用まで同一に保つ。最終成果物では Owner が確認した Task の現在保存先 ref / 版と実対象を同じ比較点で照合し、未指定なら `AskOwner` として試行を作らない。Workspace の書込み権・作成時の保存先 snapshot・ファイル名だけを最終保存先の根拠にせず、作用直前にも用途と現在 ref / 版を再比較する。中間用途では元資料・最終成果物の上書きを許さない（PR §4 Group D、IB K-H）。
- `TaskBound { scope: BackupTaskExport }` では保存済みの Backup 専用 Task の source（Started occurrence または認証済み Owner Run now）・保存済み purpose / 担当 Running Companion、Run now の Owner role・元操作の認証済み帰属、保全 owner の同じ Task / point / 準備済みの確定出力 scope・内容と回復不能な失敗 marker の不存在、Owner 選択の現在 `backup_setting` revision / 保存先、外部 Create の保持実対象、Permission owner の single-use 評価を同じ比較点と最終作用開始で照合する。point の失敗 marker の確定も同じ writer で直列化し、Task の `Failed` 確定前であっても既に失敗した point へ新しい出力を始めない。Workspace / delegation の有無を他用途の既定許可として読み替えず、保存先未選択・変更・権限や確認条件の不成立なら試行・作用なしで確認待ちまたは再評価とする。
- Backup 専用 Task の AU3 は出力試行の前後を問わず同じ master の種別 / source 比較で常に拒否する。通常 Task の AU3 / Workspace は維持する。Backup 出力 AU5 の `UNIQUE(task_id)` は P1 の `Unknown` または失敗確定の後の P2 を別 point からも拒否する。再出力は Owner が明示的に新 Task / point を判断したときだけ許す。AU15b は同じ transaction で通常 Task の種別 / source を照合し、Backup 専用 Task を通常 Agent 完了にしない。

条件がひとつでも一致しない場合はアクションを開始しません。Stopped / Deleted / deletion hold は `CompanionUnavailable`、消去による covered / 判定不能は独立した domain refusal `DataUseHeld` として `Denied` / `StalePremise` / `HeldByGlobalHold` / technical error に丸めず、`action_attempt` の挿入・外部作用ともに 0 とします。ユーザーの承認が必要な場合は実行せずに承認待ち（待機）とします。不一致となった明確な理由を呼び出し元に返し、勝手に裏でキューに溜めて後から自動実行してはいけません。

`Create / Edit` の実書込本文と既知 source lineage は AU5 の前に Action owner が確定し、比較した本文と作用に渡す本文を一致させます。重い本文の materialize・対象解決・検索や外部 I/O を transaction 内に入れず、事前の機械的判定を条件集合の世代と候補の同一性に束縛します。短い `Immediate` では authoritative な current condition / phase・完了後 provenance cut とその判定の有効性を照合して attempt と body-free な source / 順序相関を確定します。条件・候補が変われば transaction 外で再判定し、同一性や coverage を証明できなければ保留します。対象本文・検索 material・本文 hash を attempt に複製しません。`Read / List` では対象・既知 source を AU5 で照合し、実際に読み出した本文・列挙名を利用・返却・再保存する前にも現在条件の機械的 coverage を最終照合します。結果を得るまで本文を判定できないことを、未照合のデータを利用する許可にはしません。

### 8.2 アクション開始後の結果追跡（並行実行、試行ごとの比較更新）

手動管理 `Direct { companion: None, system_operation: ManualBackupExport }` は、保全 owner の一意な `Management(ActivityId) → backup_point_id` と確定出力 scope / 内容を source の受付時に保存します。AU5 はこの point の準備完了・回復不能な失敗 marker の不存在と認証済み source / Owner 指定保存先の保持実対象・Permission の現在許可を同じ SQLite master の `Immediate` で照合し、同じ source の既存出力試行がないときだけ一件を開始します。評価 ID を変えたり同じ管理要求で別 point を作ったりして P2 を通しません。作用直前にも同じ source / point の準備完了・失敗 marker 不在・保持実対象・許可を再照合し、先勝ちの変更なら作用せず元試行の無作用を客観的に証明できた場合だけ失敗へ進めます。`Unknown` は維持し自動再送しません。別の明示管理要求は新 source / point として現在の許可と旧 Unknown の二重出力リスクを評価し、旧試行を消しません。

- AU5 の durable attempt と実対象作用開始の間では、Task Cancel / revision 前進 / execution seal、権限失効・依拠 rule revision の変更、該当する Stop・削除開始を最終作用 gate が再比較します。Workspace TaskBound は現在の progress / revision / seal / workspace と保持した実対象、Direct は認証済み Owner 指示の現在性・許可対象 scope と実対象、`Some` なら同じ Running Companion、`None` なら同じ管理起点 system operation・操作・用途を確認します。双方で AU5 が single-use 消費した評価 ID の binding・依拠 rule / revision（導入済みの場合）と現在の権限、端末・費用 / 保留・消去・復元条件を作用開始点と同じ master の短い区間で直列化し、評価 ID を再消費しません。先勝ちの失効では作用を始めず、未開始を客観的に証明できる場合だけ元 attempt を `ConfirmedFailure` + `RefusedBeforeEffect` とし、証明できなければ `Unknown` のまま保持します。作用開始後の停止は best-effort であり、作用完了まで lock を保持せず、自動再実行しません。Computer Use の Host→Client 発行で何を作用開始とするかは IPC の発行 fence に接続します。
- Backup 専用 Task では Task の revision / 非 terminal・保存済み Started occurrence または Owner Run now source / point / 準備済み出力 scope・回復不能な失敗 marker の不存在・現在の設定 revision と保持した保存先実対象・担当 Companion Running、single-use 評価の消費済み binding と現在の Permission 条件を最終作用 gate でも短く直列化する。point の失敗 marker が AU5 後・作用前に先勝ちしたら外部 Create を始めず、無作用を証明できる場合だけ元試行を失敗確定し、証明不能なら `Unknown` を維持する。作用が `Unknown` の間は Task-wide completion barrier が Task 完了を止め、保全 owner の point marker と Action owner の確定度を別々に読み、外部出力を再起動時に再送しない。Action の ConfirmedSuccess だけで point の整合性成功や Task の Completed を宣言しない。
- Backup 専用 Task の出力先では保持した親ディレクトリと leaf の実対象を評価から publish まで保ち、同一保存先の writer を lock / CAS / 単一 writer で直列化する。no-clobber と atomic publication を満たさない外部ファイルや部分書込を成功として扱わず、不可逆 publish 後の結果が不明なら元試行の `Unknown` を保持する。
- 進行フェーズ（受付、開始、送信、把握）、結果の確定度（`ConfirmedSuccess` / `ConfirmedFailure` / `Unknown`）、根拠の対応、保留状態を `action_attempt` レコードに保持します。
- 確定度の更新は `(attempt_id, 期待される確定度)` を指定した比較更新（CAS）とし、事実確認が取れた確実な証拠がある場合のみ更新します。`Unknown`（成否不明）は非常に粘り強く保持し、単にキャンセルを受け付けた、通信が通った、画面に表示された、再接続された、復元されたといった間接的な出来事だけで勝手に成功や失敗に書き換えてはいけません。
- AU5 の commit は後日の作用許可ではありません。作用開始直前にも同じ本文・source・対象、Companion 付きならその `Running`、全帰属で deletion hold 不在について current condition を再照合し、該当する Stop / 削除開始との順序を同じ master の条件更新に結び付けます。比較後・作用開始前に Stop / 削除が割り込む隙間を残さず、短い同期区間内で作用開始を確定できなければ区間を解放して再照合します。開始後の外部 I/O 完了までは保持しません。新たに Stopped（Companion 付き）/ held / covered / 判定不能なら作用を止め、既存 attempt の客観的な無作用が証明できる場合だけ確定度を更新し、証明できなければ `Unknown` を保ちます。結果本文の再保存も owner の受入確定点で current condition / provenance cut に照合し、旧由来・covered / 判定不能は本文なしの事実記録に留めます。
- Client 側 Computer Use では、上記の短い作用開始区間は Host が `ClientActionCommand` を取り消せない transport に渡す直前の最終 gate と不可逆 dispatch の組を指します（HCIPC §15.2）。AU5 の試行行、open の受付、queue、ACK は開始ではありません。Stop が先なら未送信 command は 0 byte で止め、dispatch が先なら作用可能な既開始試行として追跡します。後者の Client での物理入力は Stop 通知前に始まり得るため、Client のローカル停止 fence と best-effort 中断を適用し、入力不在・停止完了を推定せず `Unknown` を維持します。Host の Stop commit と分散 Client の物理入力を原子的に直列化したものとして扱いません。Task Cancel・権限失効・評価 binding・消去条件も不可逆 dispatch 前の同じ最終 gate に含めます。
- 外部サービスから「成功」の返答があった場合でも、対象や作用の対応関係を確認した上で初めて証拠として扱います。AIエージェントの自己申告を成否の証拠にしてはいけません。
- タスクの達成やユーザーへの報告は、アクション試行の記録がDBに保存されたことを確認した上で、別のトランザクションとして安全に更新します（durable-before-adopt）。

### 8.3 競合制御のルール

- **2つの並行するアクション試行**: 各試行の開始条件チェックは短いトランザクションで直列化されますが、アクションの実行そのものは完全に並行して行えます。同一タスク・同一対象への並行実行が明示的な並行委任によるものであれば許可されます。重複実行のリスクがある再試行は、ユーザーの判断と元の試行への関連付け（`prior_attempt`）を明記した新しい試行として扱い、別ルートを使ってこっそり再実行するような迂回を行ってはいけません。
- **タイムアウト vs 遅れて届いた成功応答**: タイムアウトが発生しても確定度は `Unknown` のまま維持します。遅れて届いた成功通知は、元の試行レコードに対して `Unknown → ConfirmedSuccess` とアトミックに記録します。その結果を現在のタスクに採用するかどうかは、最新のタスク前提と照らし合わせて別途判断します。成否不明な処理に対する再試行は、重複実行のリスクや追加費用をユーザーに明示した上で、新しい試行として進めます。
- **実行とキャンセルの交差**: 実行前であれば即座に開始を阻止し、実行中であれば可能な限り停止（best-effort）を試みます。停止できなかった部分や成否不明な結果は正直に事実として記録・報告します。「キャンセルを受け付けたこと」と「安全に停止が完了したこと」を混同してはいけません。
- **権限認可と実対象解決の分離**: 対象の文字列表現が一致していることだけで認可を通してはいけません。パス解決などによって実際に特定された対象（`RealTargetRef`）を記録し、対象の解決前提が変わった場合は権限を再評価します。
- **実対象の解決と使用の結合**: `RealTargetRef` は記録用の対象記述であり、再オープンに使う capability ではありません。実行・拡張担当は検証済みの実対象（新規作成では親ディレクトリと leaf）を非永続の guard で保持し、その同じ対象を認可・AU5 開始後の作用に用います。検証後の名前差し替えで別実体へ作用する経路を許さず、競合下で同一性や Workspace 境界を保てない場合は読み取りも含め拒否します。AU5 後に検出した失敗は試行の確定度に従って記録します。
- **AU5 と Targeted Deletion の 2 順序**: 消去条件 commit が先なら AU5 の同一 `Immediate` 比較で `Create / Edit` 本文の mechanical coverage または source / 対象の coverage を検出して `DataUseHeld`（試行 0・作用 0）。AU5 commit が先なら body-free lineage を持つ `action_attempt` は already-started fact として deletion participant が列挙し、停止・縮小、作用直前の再照合、遅延結果の本文再保存防止の対象にします。開始済みを未実行へ書き換えず、効果不明を成功・失敗へ捏造しません。`Read / List` も開始後の取得本文の利用を最終照合に従わせます。

## 9. 権限・コスト・リソース消費の予約制御（reservation / commit / release）

### 9.1 並行消費による上限突破の防止

複数のリクエストが同時に「まだ上限まで余裕がある」と判断して一斉に処理を開始し、結果として上限を大幅に突破してしまうレースコンディションを防ぐため、処理開始前の「仮押さえ（reservation）」と確定時の「実績確定（commit）」を同一トランザクションで厳格に管理します。

- 実際の利用実績の記録は各機能（推論、タスク実行、外部連携など）に残し、権限・制約サブシステムは可否のルールと上限値のみを管理します。
- 処理中の枠や成否不明な消費を勝手にゼロとみなしてはいけません。上限に達したり成否不明で安全な継続が判断できない場合は、データを保持したまま安全に停止し、ユーザーの判断を待ちます。

### 9.2 仮押さえ・確定・解放のライフサイクル

```text
状態の流れ: Reserved（処理中・上限枠の仮押さえ） → Committed（実績確定）または Released（枠の解放）
※ 実績値については Reported（報告済み）と Unknown（不明）を明確に区別して保持
```

- **仮押さえ（reservation: 処理開始時のごく短いトランザクション）**: `usage_fact` テーブルに `Reserved` レコードを登録し、同一トランザクション内で「現在の上限値」と「既存の Reserved + Committed + Unknown の合計値」を突き合わせます。上限を超過する場合や成否不明枠により安全が確認できない場合はロールバックし、処理を開始しません。仮押さえの量は予想される最大消費量（上限の見積もり）とし、過小な見積もりによる上限突破を防ぎます。
- **実行（ロックなし）**: 仮押さえが完了した後のAI推論やツール実行は、ロックを保持せずに並行して行います。仮押さえを保持していることを理由に他の無関係なリクエストをブロックしてはいけません。途中で権限の失効や停止要求が発生した場合は、可能な限り処理を停止します。
- **確定（commit: 処理完了時の短いトランザクション）**: 実際の利用実績（報告された確定値、または不明の旨）を記録し、仮押さえしていた余剰枠を解放（Released）します。遅れて届いた利用量報告は元の利用記録に正しく紐付け、最新の利用可能枠を再計算します。処理中や成否不明な消費を、コンパニオンの削除や再起動、キャッシュクリアなどを理由に勝手に消去してはいけません。
- **解放（release: キャンセルや失敗時）**: 使用されなかった仮押さえ枠を `Released` に戻し、利用可能枠を回復させます。ただし、すでに外部サービスで消費されてしまった分や成否不明な消費は解放せず、事実として残します。
- **UTC window を跨ぐ dispatch**: provider/system の日額・月額枠では、reservation の commit だけを翌 window の送信許可としません。provider dispatch 開始直前の期限付き gate で予約した両 UTC window と現在時刻を照合し、間に合わない場合は送信前に止めます。provider 0 byte と旧 ticket の送信不能を証明できたときだけ予約を `Released` にし、新しい attempt / reservation で現在 window の全適用 cap と他の現在前提を再評価します。送信が始まった可能性があれば Unknown として元 window の枠を保持し、自動再送しません（[Usage / Cost / Cap §9](usage-cost-cap.md#9-provider-send-admission-の-linearization)）。
- **クラッシュにより孤立した仮押さえの復旧**: ホストのクラッシュ等で処理が中断し、`Reserved` のまま残された枠は、再起動時の再評価処理によって実態を照合し、成否不明な実績として確定（`Committed / Unknown`）させます。不明な理由を明確に管理画面等に表示し、勝手に枠をゼロに戻して上限突破を引き起こさないようにします。
- **実行中の権限失効**: 処理の実行中にユーザーが権限を取り消した場合、以後の新しい仮押さえを拒絶し、実行中の処理に対して可能な限り停止を伝えます。ただし、すでに消費が確定した過去の記録をなかったことにしてはいけません。

### 9.3 複数のタスクエージェントが並行動作する場合

- 並列に委任された複数のエージェントによるリソース消費は、同一のコスト上限枠（`CapId`）から合算して引き落とします。エージェントごとに勝手な独立枠を作ってはいけません。
- 画面監視（Observer）の消費は専用の割り当てとして記録を区別しますが、システム全体のコスト上限の合算には正しく含めます。


### 9.4 Credential の世代公開

credential owner は immutable な有効 snapshot の取得と更新を credential publication guard で制御します。取得順序は control admission → credential publication guard → SQLite とし、必要なものだけを取得します。OS store の秘密値読み書き・接続待ち・provider 応答待ちでは保持しません。確認の確定は同じ control admission 内で GUI close / session expiry と直列化します。OS candidate の保存は公開ではなく、有効 version の切替・approval sweep・既存 credential-set revision の前進・操作 outcome の commit が有効化の確定点です。同じ guard 内で準備済み snapshot を公開し、旧世代の本文・lease は保存 / inference claim の revision 比較で拒否します。provider 由来の scrub 済み stream chunk は enqueue 時点を送信開始とみなさず、application / transport の待機列を経た各 chunk の実送信開始点を同じ guard の下で更新 commit と直列化し、現在 revision・verified used-version 集合・送信前削除条件と配送可能性の durable 記録を再照合します。更新が先なら旧 proof の未開始 chunk は 0 byte とし、開始が先なら元の revision に帰属した送信事実を保持します。同じ master の単一 writer が現在性比較と配送可能性の durable commit を行い、SQLite transaction を解放したまま guard 下の ready 済み transport で有界・同期・nonblocking の first-write を実行します。OS 側が正の byte を受理するか、実際に開始した byte の不可逆な handoff を確認してから guard を解放し、enqueue・TLS buffer への格納・将来の flush は開始としません。EAGAIN 等で未開始を証明したら guard を解放して次回再比較し、部分書込・結果不明は元の送信事実として保持して自動再送しません。guard 内で `.await`、blocking OS / TLS I/O、転送完了待ちは行わず、DB transaction を I/O 中に保持しません。更新・削除開始の別 writer がこの境界を迂回して commit することも許しません。claim が先でも元 version の attempt / used-version 相関だけを保持し、送信済みとは判定しません。更新・失効が first-write より先なら旧 lease の未開始 byte は 0、first-write が先だった分だけ元 version に帰属させ、最新キーへ付け替えません。競合・失効・部分失敗・startup は [Credential publication](credential-publication.md) に従います。

## 10. 滞在先・クライアント端末の競合制御（`PresenceGeneration` による制御）

### 10.1 直列化と確定前照合

- 同一のコンパニオン（`CompanionId`）に対する滞在先の切り替えは、`SD-Presence` ドメインにより直列化し、世代番号と状態の比較更新（CAS: `expected_generation + expected_state`）を通じて `旧端末 → 移行中 → 新端末` という明確な状態遷移をアトミックに永続化します。これには遷移ログの追加と、必要に応じた復旧ヒントの更新が含まれます。
- 復旧ヒントの更新と実際の滞在権限の成立を混同してはいけません。過去に保存された接続情報から勝手に滞在を復活させてはいけません。
- 異なるコンパニオン同士の滞在先切り替えは並行して処理できます。また、同一コンパニオンであっても、画面表示、音声の再生、画面監視などの「滞在先の変更を伴わない通常の入出力処理」は、滞在ロックの外で並行して実行します。

### 10.2 競合制御のルール

- **同一コンパニオンの二重アクティブ禁止**: ホストPCが管理する滞在情報 `(companion_id, generation, active_client)` を唯一のマスターデータとし、同時に2つの端末でパートナーが「滞在（Present）」状態になることを絶対に防ぎます。複数の端末から同時に呼び出し（summon）があった場合、世代番号の比較更新に先に勝った1台のみを有効とし、後から届いた要求は受理せず再評価に戻します。端末側のローカルな状態だけに依存してはいけません。
- **端末間の移動（move）**: 切り替え期間中は「旧端末」「移行中」「新端末」「アクティブ端末なし」「停止中」「復旧待ち」の状態を厳密に区別します。移行期間中は新旧どちらの端末でも端末依存の新規処理を開始しません。移動元で実行中だった処理は安全な区切りまで完了させ、古い端末で実行していた外部アクションを別の端末へ勝手に自動継続させてはいけません。
- **通信切断時のフォールバック（disconnect）**: 一時的なネットワーク切断が発生しても、滞在記録を直ちに破棄してはいけません。疎通や排他性が確認できない間は新規処理を開始しません。正常な切断やプロセスの終了が確定した場合は、ホストPC上で利用可能なデスクトップ端末（同一マシンであることが検証され、認証とデバイス権限が確認できたもの）へ安全にフォールバック（`旧端末 → 移行中 → 新端末`）します。候補が存在しない場合は「アクティブ端末なし（`NoActive`）」とします。ホスト側の画面を勝手に自動起動したり、通常の切断を障害復旧待ち（`RecoveryWait`）と混同してはいけません。
- **再接続（reconnect）**: 端末が申告する世代番号、ホスト側の最新世代番号、実際の接続疎通、最新の権限や保留状態を突き合わせます。古い一時キャッシュや過去の承認情報だけで接続を認めてはいけません。通信状態が確認できない場合に「おそらく繋がっているだろう」と推定してはいけません。前回の会話ラウンドの入力や未提示の出力を、新しい会話ラウンドへ勝手に付け替えてはいけません。
- **ホスト再起動後の滞在復元**: 保存されている滞在記録、復旧ヒント、および現在の実際の接続・認証・排他性を突き合わせて再構成します。稼働中（Running）だったコンパニオンは、接続が確認できた場合にのみ元の端末へ自動復元し、元 Client が未接続なら PR §6.4 の RecoveryWait を保持します。再起動前が NoActive / InTransition だった場合は NoActive とします。停止中（Stopped）のコンパニオンを勝手に稼働させてはいけません。滞在状態が復旧したからといって、過去のタスクやアクションを勝手に再開してはいけません。
- **一時停止（Stop）**: 一時停止操作は「滞在の解除」として記録され、移動や呼び出しと競合した場合は常に停止操作を優先します。通信が回復したことだけを理由に勝手に再開・再配置してはいけません。
- **Stop / Deletion / Host 境界と定期到来**: scheduler の事前 `Running` read は権威にならず、AU18 は一意の発生キー・現行版・個体 lifecycle / deletion hold・Host epoch / 区間種別・restore hold を短い `Immediate` 比較点で再読込します。予定 UTC 時刻が証明済み Stop または Host 停止区間内なら `Missed`・Task なし、Stop より前の未受付回は `Unclassified`・Task なし、到来の commit が先なら `Started` と新 Task を残し、後続 Stop / Deletion は best-effort 中断を試みます。停止下端なしに crash した前 epoch の未記録回は `Unclassified`・Task なしで保護し、分類済み境界から `Missed` を作りません。再到来は一意キーの既存結果を返し、`Unclassified` の更新には停止を示す新しい durable 事実が必要です。Resume 後の現行版 due 回は新しい連続稼働期間のものだけ評価し、復旧・Run now で過去回を補完しません。削除 hold 以後の新規開始を拒否し、既存 occurrence / Task と外部作用・`Unknown` は残します。scheduler loop と外部 I/O は DB 比較点の外です。
- **端末移動と通信切断の競合（summon A→B vs disconnect）**: 両者を `SD-Presence` の順序で直列化します。先に確定した方の世代番号が最新となり、後着は新しい状態に対する再要求として評価します。二重滞在や古い処理の勝手な自動継続は決して行いません。
- **古い端末メッセージの隔離（stale Client message）**: 会話ラウンドやアクション試行、画面観測データには現在の滞在世代番号（`PresenceGeneration`）を添えておき、古い世代のデータだけを根拠に新しい処理を開始しないようにブロックします。

### 10.3 処理フローの例（滞在先の切り替え）

```text
-- §10.4 の connection 同期区間内で移行開始（短い transaction）
BEGIN IMMEDIATE;
  cur = SELECT state, generation, active_client FROM presence_attribution WHERE companion_id = ?;
  IF cur.generation != expected_generation OR cur.state != expected_state OR cur.active_client != expected_client THEN
    ROLLBACK;
    RETURN StalePresence; -- 最新前提へ自動で付け替えない
  END IF;
  CHECK Companion が Running、現在の意図・接続・許可、および generation の checked increment;
  UPDATE presence_attribution
    SET state = 'InTransition', active_client = target_candidate, generation = cur.generation + 1
    WHERE companion_id = ?;
  UPDATE relocation_hint ...; -- last_client は最後の正式帰属、recovery_destination は解除
  INSERT INTO presence_transition_log(...旧端末から移行中への遷移...);
COMMIT;

-- 移行処理（安全な区切り待ち。DBトランザクションやロックは保持しない）
-- 移動元での未完了処理を安全な区切りまで完了させ、新端末の準備を行う
...

-- §10.4 の connection 同期区間を再取得して移行完了
BEGIN IMMEDIATE;
  cur = SELECT state, generation, active_client FROM presence_attribution WHERE companion_id = ?;
  IF cur.state != 'InTransition' OR cur.generation != transition_generation OR cur.active_client != target_candidate THEN
    ROLLBACK; RETURN StalePresence;
  END IF;
  CHECK Companion が Running;
  -- 現在の connection・許可が一致する候補だけを Present にする。失われた候補なら NoActive。
  UPDATE presence_attribution
    SET state = confirmed_state, active_client = confirmed_client
    WHERE companion_id = ?;
  UPDATE relocation_hint ...; -- Present になった場合だけ last_client を更新
  INSERT INTO presence_transition_log(...移行中から Present または NoActive への遷移...);
COMMIT;
```

### 10.4 connection の現在性と presence commit

`LiveInput { authed: true }` の事前 snapshot は commit の根拠にしません。connection table owner は auth install、supersede、close、device revoke と、presence/Client-dependent command の admission を同じ短い同期区間で直列化します。`spawn_blocking` 内で connection table の lock を取得し、current `(device, incarnation, connection)` を検証したまま必要な短い SQLite compare/commit を完了します。先に snapshot を取り、lock を解放してから DB へ await する形は禁止します。本文解決・proof の準備・入出力は外で行い、最終の device 有効性確認と phase/nonce 消費は install の区間に含めます。device revoke もこの順序を守り、古い検証結果の install を防ぎます。

lock の順序は connection table → presentation memory lock → 必要なら Task execution registry → SQLite 接続です。各操作は必要な lock だけを取り、逆順に取得しません。Client-dependent operation は「prepare（本文解決・admission・expensive read・provider I/O）→ ownership 区間での currentness 検証と final commit → async continuation」に分け、ownership 区間へ入るのは最終 commit/admission だけにします。この区間で行う durable commit は同期 SQLite primitive とし、connection table を保持したまま `.await` しません。これは接続の admission と commit を結ぶ局所的な同期区間であり、Task runner や全 request を囲む global async lock ではありません。§15.2 の await 禁止は維持します。cancel や settlement は DB commit 後に lock を解放してから通知し、通知先からこの lock を逆取得しません。

`SubmitTextInput` の durable acceptance は ownership 区間内の Owner append commit です。acceptance 後に connection が失効しても Owner row は取り消しません。ただし open Round install の成功は、その後の publication の許可を意味しません。`AcceptedForRound` と `TextStreamOpen` の新規 publication は、別の短い ownership 区間で currentness を再確認したまま同期 control queue へ enqueue します（socket write は区間外）。install が拒否された場合、または publication より先に replacement が成立した場合は、受理済み work の既存 dispatch/adoption 契約を維持しながら wire stream を開かず、close も送りません。delta と final/Completed も容量待ちの後に ownership 区間内で enqueue します。既に開いた stream の Interrupted は失効通知であり、新しい stream の開始ではありません。durable command の exact retry が返す既存 acceptance の再掲も、新規 Round install / stream 開始とは区別します。

presence attach の unsolicited fact と auto-present summary も同期 enqueue 時の currentness を検証します。fact の commit → fact enqueue → summary の順序は維持し、fact の送信失敗時は summary を送りません。replacement が enqueue に先行した場合、durable presence fact は保持しつつ旧 connection への publication は省略します。SelectTask と Task ref/cursor mint は既存の memory ownership 区間、ResumeTask は既存の guarded owner commit を使い、replacement/close の単一 lifecycle cleanup を迂回しません。

presence の begin は `(companion, state, active_client, generation)` を CAS し generation を 1 進め、hint と遷移ログを同時に記録します。confirm は同じ InTransition generation と移動先候補を比較し、その場の current authenticated connection と device 利用許可が一致する場合だけ Present にします。候補が失われたら NoActive、前提が変わっていれば StalePresence で書込なしです。confirm は同じ遷移内なので generation を再度進めません。再度の begin と startup invalidation は新 generation を発行します。

| 接続と帰属の競合 | 期待結果 |
|---|---|
| C1 auth → C2 auth → C2 close、C1 が残存（#1384） | C1 は Superseded。current は空。Present を維持せず、利用可能な別の Host-local Client または NoActive へ fallback |
| C2 auth 後に C1 が capability/auth を再送（#1385） | StaleConnection。新 nonce を発行せず、C2 の current を変更しない |
| C1 close 通知の処理前に C2 auth | commit 区間で C2 の current を見て C1 の close による fallback は行わない。presence は C2 が使う現在の帰属のまま |
| fallback begin が先、同じ device が再認証 | 既に始まった通常切断遷移を認証で取り消さない。NoActive または選定済み fallback へ確定。再接続側は新しい summon が必要 |
| fallback target の認証失効・close と confirm | confirm より前なら Present にしない。confirm 後なら新しい切断遷移が帰属を外す。古い bool で Present を確定しない |
| Owner summon / Stop と restart recovery | generation CAS に従う。Stop の lifecycle が確定した後は recovery/summon を拒否。別 Client の summon が先なら元 Client の late auth は復旧しない |
| DB 書込失敗・遷移途中の crash | 遷移が確定したとは返さない。残った InTransition では admission を拒否し、次の startup は NoActive へ置く |

fallback 候補は current authenticated、[IPC §10.5](host-client-ipc.md#105-samemachine-の根拠) の `SameMachine` 検証、必要な device 許可を満たす別 Client に限定します。ローカル専用 listener と現在のローカルトークン・Host 起動世代の検証結果を connection に束縛し、IP や保存済みの通信種別で代用しません。複数なら `ClientId` のバイト列昇順で最初を選び、confirm でも同じ connection の現在性・有効性を再照合します。候補なし・確認不能なら NoActive とし、UI を起動しません。新しい優先端末設定や fallback manager は設けません。

### 10.5 未伝達 ACK は選択した事項だけを確定する

発生元 fact と未伝達登録は PR §4.6 の同一 transaction です。表示に使う source facts を read transaction で読み、同じ Task を現在の report へまとめます。receipt に含めるのは、その report に実際に含めた通知 ID だけです。後から追加された行や、同じ Task の未選択ページを ACK の対象にしません。

各 selected ID の提示済み判定は、receipt に載せた要約本文全体の画面提示、または音声再生完了のいずれかです。音声だけが失敗してもテキスト提示が成立すれば確定できます。どちらも未成立・不明なら Unknown を保ちます。部分提示の batch 全体を成功 ACK にせず、提示済みの事実と Task の作用・承認を混同しません。

提示開始と ACK は connection/receipt の同期区間を経て、行ごとの status と source の存在・current erasure 条件を同じ DB transaction で比較します。成功 ACK は選択行だけを Presented にし、重複 ACK は書込なしの AlreadyPresented とします。旧 connection、失効した receipt、別 Round/generation の ACK は状態を変えません。古い receipt の Failed/Unknown で、新 receipt が確定した Presented を戻しません。

提示開始は bounded page/excerpt/frame fitting を prepare した後、同じ ownership 区間内で Pending→PresentationUnknown の同期 CAS、成功した selected 集合の確定、receipt/cursor/subscription install を行います。既に Unknown の再提示は status を変更しません。StaleSource/technical error の行は frame と selected の両方から除外します。replacement が先なら durable mutation も memory install もゼロ、commit が先なら cleanup は memory だけを落とし durable Unknown は再提示対象として残します。

ACK は receipt consume を受理点とし、その bounded selected 集合への同期 durable mark まで同じ connection ownership 区間に含めます。consume と durable mark の間に replacement は成立できません。ConfirmPresentation は round/companion 解決と最大50行の候補 read の後、ownership 区間内で currentness と expected status を比較して同期 CAS します。旧 connection の observation は wire reply を追加せず、durable mutation ゼロで終わります。いずれの DB commit 区間も blocking pool 上で実行し、memory/table lock を保持した async DB await は行いません。

登録 commit 後の wakeup は配送保証を担いません。購読開始では wakeup receiver を設置してから durable backlog を読み、走査中の commit は走査済み挿入キーより後の次 pass に回します。接続後の new fact を coalesced wakeup で知らせ、通知落ち・receiver lag は同じ走査下限から durable query を再開します。Unknown / Failed を新着のたびに再送しません。restart では DB の未提示行を読みます。receipt や送信 queue の消失は報告漏れになりません。

## 11. 個人データ完全削除との競合（Targeted Deletion との race）

本節の設計は [個人データ完全削除（targeted-deletion.md）](../critical-areas/targeted-deletion.md) および永続化設計を前提とします。全ドメインをひとつの巨大なトランザクションで長時間ロックするような強引な手法は使いません。

### 11.1 先行永続化と各箇所での条件照合

- **消去条件の先行永続化**: 削除操作と具体的な消去条件（`operation + erasure_condition`）を、実際の削除処理の開始に先立ってDBに永続化します（durable-before-enforce）。
- **各受入箇所でのロックフリー照合**: 会話履歴、タスク、長期記憶、アクション、未伝達メッセージ、キャッシュ、端末側の一時コピーなど、すべてのデータ受入箇所は、データを保存・採用する確定直前に、現在進行中の消去条件（`ErasureConditionRef`）をロックなしで照合します。
- **送信・利用開始点での照合**: 保存・採用だけでなく、外部への送信・利用を開始可能にする確定点でも現在の消去条件を照合します。Task Agent の provider I/O は、推論試行 claim（AU14）と同一の短い `Immediate` トランザクションで、送信する論理入力の解決済み canonical source 相関（採用目的・全採用指示の `origin.source`。本文・hash を含まない）が現在の消去条件に覆われていないことを確認した後だけ開始します。claim と消去条件の durable 化は同一 SQLite master で直列化されるため、「消去条件 durable → claim 拒否（provider 0 バイト、`DataUseHeld`）」と「claim commit → attempt が already-started use」の 2 順序以外は存在しません。claim 前に materialize 済みの本文・scrub 済み prompt は、この照合を通らなければ送信されず、拒否時は cache・retry に残しません。canonical current-condition store は Stage 4 erasure-currentness foundation が導入し、Stage 6 の Targeted Deletion producer は同じ store へ durable-before-enforce で active condition を投入します（別の gate / currentness registry / source correlation を作りません）。active condition が 0 件の場合の「被覆なし」は store を実際に照会した authoritative な結果であり、`NoDeletion` / `ErasureConditionRef::none()` / generation 0 のような sentinel ではありません（V-11 Case 14・15）。
- **Action の利用開始と結果受入**: AU5 は Action owner の `Create / Edit` 実書込本文の機械的 coverage と既知 source lineage・操作対象に関わる coverage を、attempt 挿入と同じ master の短い `Immediate` 比較で判定します。`Read / List` の対象・既知 source も同じ gate を通し、取得した本文・名前の利用前にも機械的に最終照合します。covered / 不明なら独立した `DataUseHeld`（試行・作用 0）。AU5 が先に commit した場合も作用直前と結果再保存時の current condition / provenance cut を再照合し、deletion participant は durable attempt を already-started use として追跡します。AU5 開始と deletion start の同一 master 上の順序を、事前の権限判断やメモリ上の cached 判定で代用しません。

- **本文 gate と旧由来の順序**: 上記 `data_use` 照合に加え、AU14 の同じ比較点と provider dispatch 前の gate は送る論理本文全体（省略後の Action / Workspace transcript も含む）を機械的 current condition と比較します。source 未収録の本文を source coverage の空集合で許しません。削除開始と最終 dispatch は直列化し、claim が先で dispatch 前に条件が durable なら再照合して covered bytes を送らず、claim 事実は既存 attempt のまま保持します。AU15a は同じ SQLite master 上の短い `Immediate` commit で source / mechanical condition・完了後 provenance cut と result 本文を照合し、covered / 判定不能 / 旧由来の結果を body-free `erasure_held` 到着・seal にします。`AU15a → deletion start` なら保存済み本文は sweep / 残存検証へ、`deletion start → AU15a` なら本文は保存せず相関だけを残します。`deletion completion → 旧 AU15a` でも durable cut で拒否します。completed operation の検索材料・本文 hash は残さず、cut と attempt / delegation / Action の durable identity / 順序・source lineage を比較します。新 Owner 提供の入力は新しい由来として扱います。cut 判定が失われた場合は本文を保留します。provider / Action I/O 待機中は SQLite transaction や Task-wide mutex を保持しません。

dispatch gate の比較後に削除開始が割り込んでから bytes を渡す TOCTOU は許しません。ready 済み transport に対し、同じ writer の短い共有 guard で現在の condition・費用・権限を照合し、配送可能性を durable commit して SQLite transaction を解放した後も guard を保持して、有界の同期 nonblocking first-write を試みます。OS 側の正の byte 受理または実際に開始した byte の不可逆 handoff が成立したときだけ送信開始として guard を解放します。未開始が確定したら guard を解放して待ち、元の予約・ticket を保持して次回すべての現在条件を再照合します。元 ticket の未送信と送信不能を客観的に確定して fence した場合だけ `NotSent` と予約 `Released` を同一 commit で確定します。結果不明や部分開始は未送信とせず元の試行へ帰属させ、後続 byte を再 gate できなければ送信不能にします。ネットワーク完了まで guard / transaction を保持せず、開始済み send は already-started use として停止・縮小および遅延結果検査の対象にします。dispatch gate と deletion start の直列化は同じ master の condition 更新と結合し、プロセス内 mutex だけで別プロセス writer を見落とさないようにします。
- **完了の確定基準**: すべての参加コンポーネントにおける局所削除の完了、実際の残存検証、削除期間中に到着したデータの取り込みが完了した後、検索用トークンを完全に破棄・復元不能化し、その成功を確認して初めて全域の完了をアトミックに確定します。検索トークンが復元可能な間は完了とみなさず、`finalizing`（最終処理中）として未完了状態を維持します。
- 削除処理、残存検証、トークンの完全破棄が完了していない段階で、ユーザーに「削除完了」と表示してはいけません。

### 11.2 競合制御のルール

- **新しい情報形成 vs 削除区間**: 削除期間中に再到着したり再生成された対象データも、すべて同じ消去対象とします。削除処理の最中に届いたからといって「新しい体験データ」として勝手に特別扱いして残してはいけません。
- **遅延到着した結果 vs 削除区間**: 時間がかかって遅れて届いた処理結果についても、現在の最新条件に加えて消去条件を突き合わせます。削除対象の活動に紐づくデータは破棄するか元の履歴記録に留め、現在の会話への採用や再保存を行いません。
- **長期記憶の更新 vs 削除**: 記憶の定着処理を確定する際、現在の記憶リビジョンやスコープ制約に加えて、消去条件を照合します。削除前の古い根拠データだけを頼りに記憶を勝手に再形成してはいけません。
- **キャッシュや検索インデックスの再構築 vs 削除**: キャッシュやインデックスの無効化キーに消去条件を含めます。「キャッシュにヒットしたから」といってセキュリティ制約の確認を省いてはいけません。古いインデックスから過去のデータや権限を復活させてはいけません。
- **タスクや推論結果の到着 vs 削除**: タスクや推論の結果を現在の状態へ取り込む前に、必ず消去条件を照合します。削除対象となった情報を利用して実行中だった処理は、可能な限り速やかに停止するか対象外スコープへと縮小します。
- **残存検証と完了確定**: 一部のコンポーネントが処理を終えたからといって、システム全体の削除が完了したわけではありません。すべての参加者の報告、機械的な残存検証、検索トークンの復元不能化が揃って初めて全域完了とします。
- **最終検証と受入の競合**: finalizing の開始と text-bearing 受入の確定を直列化し、検証後から条件解除まで対象本文の未検証な新規採用を禁止します。受入が先に確定して検証の対象から漏れた場合は検索材料破棄前に新 sweep へ戻し、finalizing が先なら対象本文を保留・破棄します。外部作用の客観的事実の記録は継続できます。
- **最終処理中（finalizing）でのクラッシュ**: 各コンポーネントの削除や検証が終わっていても、検索トークンがまだ復元可能な状態であれば削除は未完了です。再起動後は通常処理にトークンを使い回さず、トークンの破棄と完了確定の処理のみを再開します。
- **削除完了後に遅れて届いたデータ**: 削除完了後に届いたデータが、削除対象となった過去の試行やタスクに紐づくものであれば、再保存を確実に阻止します。一方で、削除完了後にユーザーが新たに提供した情報は、新しい活動データとして正しく区別して受け入れます。単なる文字列の一致だけで新旧を雑に判定してはいけません。

### 11.3 境界参照トークン・操作ID・参加状態の利用

- `ErasureConditionRef(operation, sweep, valid_interval)` を各受入箇所に配布し、データ保存時の突き合わせ用インデックスとして使用します。
- `DeletionOperationId + DeletionSweepGeneration` を削除区間の識別子とし、削除区間を跨ぐ可能性のある処理やキャッシュ、端末側の一時コピーに付与して管理します。
- 参加コンポーネントごとの進捗や検証結果はDBに永続化し、システム全体の完了判定の材料とします。

## 12. バックアップと復元との競合（Backup / Restore との race）

本節の設計は [バックアップと復元（backup-restore.md）](../critical-areas/backup-restore.md) および永続化設計を前提とします。

### 12.1 ステージング領域・運用系・マスターデータ切り替えの分離

- **ステージング領域での検証**: リストア復元を行う際は、独立したステージング領域（別ファイル等）で整合性の照合・検証を先行させます。検証が完了した後、復元世代番号（`restore_generation`）とマスターポインタをアトミックに切り替えます。
- **二者択一の原則**: 切り替えの前は「復元前の正常データ」がマスターであり、切り替えの後は「復元されたデータ」がマスターとなります。両者が中途半端に入り混じった第3の混合状態を決して作りません。
- **読み取り処理を全面停止しない**: 復元の準備中であっても、通常の読み取り処理はマスターポインタ切り替えの瞬間まで復元前の正常データを読み続けることができます。切り替えの瞬間だけごく短い排他制御（`SD-Restore`）を行い、ポインタを切り替えます。切り替え後は、新しい復元データと「復元後の保留状態」のもとで読み取られます。

### 12.2 競合制御のルール

- **復元準備中の通常データ変更**: ステージング検証を行っている間、通常のデータ変更は復元前の正常データに対して通常通り書き込まれます。ステージング中の復元用データにそれらが中途半端に混ざることはありません。
- **復元前 cap の最終照合**: ステージング中の旧マスターへの cap 更新も上記の通常変更です。切替前に durable に準備した非秘密の復元安全条件には旧マスターの cap revision・有効 / 解除済み状態、実課金先・scope・limit・対象 UTC window の由来を束縛します。`SD-Restore` の最終公開点では、旧マスターの現行状態・信頼できる UTC window を、cap 更新と共通の単一 writer / 公開境界で短く再照合します。準備後に cap の引下げ・解除・対象変更・window 交代があれば、古い安全条件と hold のまま切り替えず、短い区間を解放して現行状態から準備し直します。再照合からポインタ切替まで cap 更新を割り込ませず、切替後の書き込みは新マスターへだけ行います。旧マスターの状態が読めなければ早期 snapshot を現在値と推測せず、該当し得る scope × window を保留します。切替前 crash は旧マスターと未公開 context の再照合から、切替後 crash は新マスターの同一世代の context・gap・hold の検証から復旧します（UCC §15.1、PR Group F / §9）。
- **現行 deletion の最終照合**: ステージング中の削除開始・sweep / participant 進捗・finalizing / completion も旧マスターに commit できます。旧マスターの active operation identity・sweep / phase・condition set・hold・provenance cut / 旧由来相関・材料の非秘密参照を置換対象外の durable recovery context へ準備し、復元候補には同じ現行条件と新 sweep の未検証状態を付けておきます。OS material の照合、復元対象の機械的 scan、重い DB / staging 書込・fsync は比較区間の外です。OS 材料の破棄や参照の付け替えも開始前に旧マスターの材料状態を durable に更新し、準備済み参照を stale にします。`SD-Restore` の最終公開点で deletion start / finalizing / completion と共通の writer / 短い公開境界に入り、旧マスターの `deletion_restore_revision` を準備時の active set・phase / 材料参照に束縛された版と再比較します。変化していれば切替を abort し、境界を解放して現行値から再準備します。一致した場合だけ新マスターの condition・hold・cut / 相関・材料参照と復元世代・ポインタを一体で公開し、以後の deletion 更新は新マスターへ向けます。別ドメイン mutex を重ねず、旧マスターの DB 行の欠如だけを OS item の消去証拠にしません。照合・公開・crash 後の正本を証明できなければ対象範囲を hold して restore を延期します（PR Group J / §9）。
- **deletion 比較の境界**: 準備済み active set は Group J の `deletion_restore_revision` に束縛し、削除開始・進捗・条件・材料の変更は同じマスターの commit でその改訂を前進させます。OS 材料への破棄作用は先に pending を durable にして版を前進させます。最終公開点では旧マスターのこの 1 行を同じ writer 区間で比較し、全 operation や本文を走査しません。版の欠如・不一致・材料状態の不明があれば公開せず、旧マスターの現行値から再準備します。
- **古い実行結果と復元データの混同防止**: マスター切り替え前に開始された推論やツールの結果が切り替え後に遅れて到着しても、復元世代番号の照合によって検出され、新しいマスターデータへ誤って混入することを防ぎます。復元前の古いセッションや古い同意情報だけを根拠に、外部通信やタスクを自動実行してはいけません。端末側の一時キャッシュでホスト側の復元データを上書きしてはいけません。
- **復元世代番号の照合**: 復元を跨ぐ可能性のある参照（タスク、ルール、同意、未伝達メッセージなど）には、`RestoreGeneration` を付与しておきます。古い世代番号の参照だけを頼りに復元後に処理を継続してはいけません。
- **復元後の全域保留（global-operation hold）**: 復元が成立した直後は、新規アクションの実行や外部送信を安全のために保留状態とします。「復元が完了したこと」と「処理の再開を許可すること」は別個に扱います。タスクや定期実行、外部通信の自動処理は一旦保留され、ユーザーが管理画面等で内容を確認した上でまとめて有効化できるようにします。
- **未完了の削除処理との交差**: 復元が行われたからといって、進行中だった個人データの完全削除を勝手に帳消しにしてはいけません。§12.2 の最終比較で公開した現行 condition は、復元後の本文利用・受入と再 sweep / 残存検証に適用します。旧 participant の完了は復元された本文の検証ではなく、条件や材料が証明不能なら対象範囲を hold します。切替前に完了済みだった削除と明示復元の関係は PR §9.4 に従います。
- **バックアップ作成との交差**: バックアップファイルの永続化が完了したことを確認してから、バックアップ成功マークを記録します（durable-before-mark-success）。未完了の削除や障害復旧と重なる場合は、不整合なバックアップを作成しないように保護します。

## 13. ファイルシステムとDBの境界（Filesystem + DB の境界）

ホストPC上の永続化データ（D1/D2/D3）は単一の SQLite ファイルで管理され、大きなファイル本体（音声や画像、添付ファイルなど）はファイルシステム上に保存されます。データベースのトランザクションだけではファイルシステムとの整合性を完全には担保できないため、以下の確実な手順を守ります。

### 13.1 安全な保存パターン

内部で保持するファイル本体、バックアップファイル、復元用ステージングファイルについては、以下の順序を徹底します：

```text
1. 保存先ディレクトリが新規作成なら、その作成エントリを持つ親ディレクトリ（新規作成した祖先も含む）を永続化する
2. 一時ファイルへの書き込み（同一ファイルシステム上の一時パスへ書き込む）
3. ファイル本体の永続化（fsync 等によりディスクへ確実に書き出す）
4. 公開保護（publication guard）の確立（この保存処理と定期クリーンアップ処理を同期させ、対象パスを掃除候補から除外）
5. 既存の参照先と衝突しない新しい名前へアトミックリネーム（既存の参照先ファイルを上書きしない）
6. リネーム先ディレクトリの永続化（名前の公開を電源断後も保持できることを確認する。異なるディレクトリ間なら双方の変更を永続化する）
7. DBポインタの登録（ミリ秒単位の短いトランザクションで、ファイルの参照、由来、用途をDBに記録）
8. 公開保護の解除（DBポインタの永続化が完了したことを確認した後に解除）
9. 不要ファイル掃除（orphan cleanup: 起動時スキャンや定期的整理により、DBの参照もアクティブな公開保護も持たない孤立ファイルのみを削除）
```

この順序の核心は、**「保存先ディレクトリとファイル本体・公開名の永続化 → 公開保護のもとで DB ポインタの登録 → 画面への可視化」**（durable-before-visible）という流れを守ることです。リネームの原子性だけでは、電源断後に公開名や新規作成した保存先ディレクトリが残る保証にはなりません。プラットフォームで必要なディレクトリ同期または同等の永続化手段が失敗した場合は、DBポインタも成功マークも確定しません。
DBへの登録を先行させて「DBにレコードはあるがファイルが存在しない」という破損状態を作ってはいけません。また、定期クリーンアップ処理が「まだDBにポインタが書かれていないから」という理由だけで、リネーム直後の公開処理中のファイルを誤って消去してしまう事故を、公開保護によって確実に防ぎます。

### 13.2 各フェーズでのクラッシュ時に残ってよいもの・残してはいけないもの

| クラッシュが発生したタイミング | 残ってよい安全な状態 | 残してはならない危険な状態（可視化の禁止） |
|---|---|---|
| **一時ファイルの書き込み前・書き込み中** | 破損した一時ファイル（次回の起動時クリーンアップで安全に削除されます）。既存のDBとマスターファイルは完全に無傷です。 | 新しいポインタの登録、成功マークの付与、画面への完了表示。 |
| **一時ファイルのfsync後・リネーム前** | 隔離された一時ファイル（公開保護中であればクリーンアップされず、クラッシュ後は掃除対象となります）。マスターファイルは無傷です。 | 新しい内容がマスターとして確定されること、成功表示。 |
| **リネーム後・ディレクトリ永続化前／DBポインタ登録前** | まだDBから参照されていない新しいファイル。正常時は公開保護によってクリーンアップから保護されます。クラッシュした場合は孤立ファイルとして安全に削除または復旧されます。既存のDBポインタは別名の古いファイルを指したままです。 | 公開処理中のファイルがクリーンアップで消去されること、参照のないファイルがマスターデータとして扱われたり成功表示されること。 |
| **DBポインタ登録後・画面表示前** | 新旧両方のファイルと、新しく登録されたDBポインタ。クラッシュ後の再起動時は新しいポインタを基準として正常に再構成されます。 | クリーンアップによって新しいファイルが誤って削除されること、古いポインタへ中途半端に巻き戻ること。 |
| **バックアップファイルの保存後・成功マーク登録前** | まだ成功とマークされていないバックアップファイル（再起動後に再試行または削除されます）。過去の正常なバックアップは無傷です。 | 各部分のファイルコピーが終わっただけで「バックアップ成功」と誤認・表示すること。 |
| **復元のステージング検証前・マスター切替前** | 隔離されたステージングファイル（既存のマスターデータは無傷）。復元操作は未完了（pending）のままです。 | 中途半端に置換されたデータがマスターとして扱われること、外部処理が自動開始されること。 |
| **復元のマスター切替後** | 新しいマスターデータと復元後の保留状態。切り替え前の旧マスターデータは、監査やロールバック用の隔離コピーとしてのみ保持できます。 | 新旧データが入り混じった状態、ユーザー確認前の権限の先行復活、古い外部処理の混入。 |

- 定期クリーンアップ処理は、DBポインタやバックアップ情報から参照されているファイル、および現在まさに保存処理中（公開保護中）のファイルを絶対に削除しません。削除の直前には、同じ同期境界のもとで「参照がないこと」「公開処理中でないこと」を二重確認します。
- 全データ初期化（Reset）の途中でクラッシュした場合でも、古い一時キャッシュや途中データから勝手に復活しないよう保護します。
- APIキーなどの認証情報（Credential）や外部ワークスペースの実体は、上記の通常のファイル管理とは明確に分離して安全に扱います。

## 14. 処理キャンセルの多層防御（Cancellation の分離）

「キャンセルを受け付けたこと」と「実際の処理停止が完了したこと」は全く別の事象です。非同期タスクの future が drop されたことだけを理由に「停止が完了した」とみなしてはいけません。以下の5つの層を明確に分離して制御します：

| キャンセルの層 | 意味 | 実装メカニズムの目安 |
|---|---|---|
| **1. キャンセル要求の受付（Request）** | ユーザーの指示、管理操作、権限失効、保留によって「処理を止めてほしい」という要求を受け付けた段階。 | タスク単位の中断は、作業担当の `cancel_task`（AU16）が `task.progress` を非 terminal から `cancelled` へ CAS した commit を受付の durable な事実とします。メモリ上の cancellation token・`Notify`・Future drop・停止シグナルの送信は受付の権威ではなく、再起動で失われ得ます。AIモデルの返答やエージェントの正常終了を待たずに即座に受理し、既に受付済みの再要求は会話 source の帰属が成立し同じ Task の中断への後続の明示撤回・置換がなければ `AlreadyCancelled`（冪等）、明示撤回・置換が先なら `Superseded`、曖昧なら `NeedsClarification` で返します。無関係な後続入力だけでは受理済み cancel を取り消しません。 |
| **2. 内部計算の協調停止（Computation）** | ホストPC内部で実行されている推論、集計、テキスト生成などの処理を中断させる段階。 | `Notify` や cancellation token を用いた協調的キャンセルと、`select!` やタイムアウトによる待機の打ち切り。非同期待機中にロックを保持しません。durable な `cancelled` の commit 後に送るベストエフォートの信号であり、drop された Future を停止完了の証拠にしません。in-flight の provider 待機を打ち切るローカルな協調停止トークン（`DispatchAbort`）も canonical state ではなく、正確な停止契約は推論境界が担います: dispatch は claim を試みる前の fast-path で abort を観測した場合は claim を行わず attempt も利用実績も残さず `Aborted` を返し、fast-path 通過後に abort が発火して claim が durable に `Started` と確定した attempt は相関を保持し、最終 dispatch gate で 0 byte と旧 ticket の送信不能を証明した場合に限り `NotSent` と予約 `Released` を同時に確定します。証明できなければ不確定利用実績（計測不明の usage fact）を記録してから `Aborted` を返します。claim 自体が拒否された場合（非 `Started`）は通常の pre-send refusal であり、`Aborted` へ写しません。呼び出し元が dispatch future を drop してこの記録を skip することは許されません。`Aborted` は provider 要求・外部作用が停止したことを意味しません。 |
| **3. 外部作用の停止試行（External Effect）** | ツール、MCP、プロバイダ通信、OS操作などの外部への働きかけを可能な限り停止させる段階。 | 実行機能からの停止シグナルの送信や通信切断。すでに実行されてしまった操作や成否不明な状態は正直に記録・報告し、別端末等で勝手に再実行しません。 |
| **4. 遅延結果の隔離（Result Arrival）** | 停止要求の後に遅れて返ってきた処理結果を安全に取り扱う段階。 | 遅れて届いた final result も、まず安全な本文または body-free held identity の 1 回だけの到着 record と、同じ不分区間での delegation の execution seal として durable 化し、採用判定では元の依拠リビジョンと Action 試行の履歴にのみ記録し（`task_result` 1 行と検証済み attempt 相関）、現在 Task への自動採用や後続処理の自動開始を決して行いません（`RecordedToOriginalOnly`、消去 held は `ErasureHeld`）。`Unknown` は維持します。seal により、その execution からの新規 inference claim / Action 開始は以後拒否されます。タスク中断（AU16）が durable な `cancelled` を確定している場合、`adopt_result` は同一の不可分な比較照合で `task.progress` が terminal であることを確認し、attempt 相関の記録に留めて現在 Task を変更しません（cancel 専用の marker 条件を追加しません）。 |
| **5. 状態確定の禁止（Commit Prohibition）** | キャンセルされた処理の新規開始と、現在 Task への採用・lifecycle 前進（確定データの書き込み）を防ぐ段階。already-started activity の事実記録は禁止しない。 | 確定（commit）直前の前提再照合により、タスクリビジョンや保留状態の食い違いを検知し、キャンセル後の書き込み要求を安全に拒絶（`StalePremise` / `HoldActive` / `TaskTerminal` / `ExecutionSealed`）します。委任・steering・推論試行 claim・Action 開始では、current な `task.revision` と purpose identity（該当する境界）に加えて `task.progress` の非 terminal を同一の不可分区間で照合し、terminal（`Completed` / `Failed` / `Cancelled`）は `TaskTerminal` として書き込み・provider I/O・外部作用を行いません。推論試行 claim（AU14）と Action 開始（AU5）はさらにその delegation が seal 済みでないことを同じ不分区間で照合し、seal 済みは provider I/O・書き込み・外部作用を行いません。結果採用・完了確定では、到着 record 済み result、delegation（execution lifetime）から列挙した authoritative set と claim の完全一致（seal 時点で membership 固定）、現在 `task.revision` と purpose identity、`task.progress` の非 terminal、依拠 Action 試行の `Unknown` が無く確認済み無作用失敗の非依拠が検証済みであること、同じ `TaskId` の全 revision / 全 delegation に `Unknown` が無いこと（Task-wide completion barrier）を同一の不可分区間で照合し、revision 不一致・terminal は `RecordedToOriginalOnly`、result-local の `Unknown` / 非依拠が検証できない失敗、または barrier の `Unknown` が残る場合は `WithheldByEffectFacts`、相関不一致は技術的エラーとして現在 Task を完了しません。seal 後に確定度が客観的証拠で進展した場合は同じ result の再評価で完了が成立し得ます。cancel が禁じるのは新規 work の admission（新規委任・steering・推論 claim・Action 開始）と現在 Task への採用・lifecycle 前進（`adopted_revision` 刻印と terminal CAS）だけであり、already-started activity の事実記録（AU15a の到着 record と execution seal、AU15b の検証済み `task_result_attempt` 相関、Action certainty の CAS、利用実績・未伝達登録など）は cancel 後も許可します。 |

- キャンセル要求の伝達、新規処理の開始禁止、状態の確認は、外部プロセスの終了や全タスクの完了を待たずに即座に行われます。
- ホストPCのシャットダウンや再起動の際も、実行中だった処理や成否不明な状態を正しく保全し、「ホストが終了したから外部の処理も消滅しただろう」と勝手に推定してはいけません。失われるのはローカルの Future・cancellation token・停止 handle・停止試行の進捗だけであり、開始済みの provider 処理・外部作用が停止したとは推定せず、durable な attempt と `Unknown`、遅延結果の帰属を維持します。

## 15. ロック設計とデッドロック回避（Locking と deadlock 回避）

### 15.1 ロックの所有権と粒度

- **データベーストランザクション（`Immediate`）**: 前提の比較とデータの書き込みをアトミックに行うための、ミリ秒オーダーのごく短い排他制御です。非同期の `.await` や外部通信、AI推論をトランザクションの内部で実行してはいけません。
- **Action 消去 gate**: AU5 前に実書込本文・既知 source・操作対象を確定し、必要な重い materialize / scan は transaction 外で行います。短い `Immediate` では同じ候補と current condition の世代・phase・coverage を検証して attempt を commit します。照合材料の変更・判定不能は fail closed です。作用直前の再照合と削除開始の順序も canonical master と結び付け、ローカル mutex だけでは別 writer の condition commit を見落とすため許しません。ファイル I/O や外部結果の待機を transaction 中に行わず、結果本文の保存は別の短い受入比較点を使います。
- **機能ごとのローカルミューテックス / メールボックス**: タスク単位（`SD-Task`）、コンパニオン単位（`SD-Presence`）、削除完了集約（`SD-Deletion`）、復元切替の瞬間（`SD-Restore`）の順序制御のためにのみ使用します。他のドメインのミューテックスと二重に抱え込むような多重保持は禁止します。
- **復元切替と cap 更新**: プロセス内の `SD-Restore` だけでは別 writer の cap commit を防げません。§12.2 の最終 cap 比較とポインタ切替を同じ writer / 短い公開境界で直列化し、先行した cap 変更は新しい安全条件へ反映、後続する変更は新マスターで再判定します。ステージング検証・context の durable 作成・OS I/O・`.await` をこの区間に入れず、別ドメインのミューテックスを重ねません。
- **復元切替と deletion 更新**: `SD-Restore` と `SD-Deletion` の mutex を同時保持して整合させるのではなく、§12.2 の現行 active set / phase / 材料参照の最終比較とポインタ公開を deletion start / finalizing / completion の writer / 公開境界で直列化します。sweep・participant の前進も準備した安全条件を stale にし得るため、依拠する現行 revision を同じ境界で確認します。旧マスター側の更新が先なら区間外で再準備し、切替が先なら新マスターだけへ更新します。重い scan・OS / DB / ファイル I/O と `.await` はこの区間へ持ち込みません。cap の最終比較も同じ公開点で行い、ドメイン別 mutex の多重取得を要求しません。
- **ファイルの公開保護（publication guard）**: ファイルの保存リネームからDBポインタ登録完了までのごく短い間、定期クリーンアップ処理との排他を行うためにのみ使用します。システム全体のグローバルロックとして流用してはいけません。
- **credential publication guard / control admission**: 第9.4節の専用境界です。ファイルの公開保護や domain mutex を流用しません。確認の失効と credential の確定を結ぶ明記された順序だけで組み合わせ、保持中に `.await` や blocking 外部 I/O は行わず、送信 owner の ready 済み transport への有界 nonblocking first-write だけは短い publication gate に含めます。SQLite transaction は first-write の前に解放します。
- **キャンセル通知シグナル（`Notify` / token）**: 処理を中断させる合図を送るためのものであり、排他制御のロックではありません。
- **ファイルシステムロック（例: `ene-core.lock`）**: プロセスの多重起動を防止するためなどの大枠の保護に限り使用し、通常のデータ書き込みの直列化には流用しません。
- システム全体を止めるようなグローバルロックや、全ドメインを包括する長大なロックは一切設けません。

### 15.2 ロック取得の順序と await 中の保持禁止ルール

- **鉄則: 非同期待機（`.await`）を跨いで DB トランザクションやドメインミューテックスを保持してはならない**。前提の読み取りと確定直前の比較を分離し、時間のかかる非同期処理はその外側で行います。
- **複数ドメインの比較が必要な場合**: 複数の非同期ミューテックスを同時にロックするのではなく、**単一の短い DB トランザクション内で決められた順序でデータを一括読み取り** します。読み取り順序を統一（例: 復元世代 → 削除条件 → タスク/委任 → 滞在先帰属 → コスト上限 → アクション試行）し、条件分岐によって逆順でアクセスすることを禁止します。
- メールボックス（チャンネルの受信ループ）の内部で、他のドメインのミューテックスやDBトランザクションを非同期待ち（await）してはいけません。
- イベントのブロードキャスト（`broadcast`）は画面への表示やログ通知のためにのみ使い、権限の確認やコミット順序の決定などの重要な整合性制御の根拠に使ってはいけません。

## 16. インターフェースが運ぶべき並行特性

各サブシステムの境界面において、並行処理の安全性を保つために決して落としてはならない必須プロパティを定めます。「暗黙的に最新のものを使う」といった曖昧な指定は禁止し、必要なトークンが欠落している場合は安全のために処理を受け付けません。

| 境界 | 確実に引き渡すべき並行特性・プロパティ |
|---|---|
| **タスク化・委任・追加指示・結果統合・中断** | `(task_id, 期待されるtask_revision)`、目的や追加指示の前提、委任スコープのコピー、ワークスペースの有効性、結果 identity と本文（finalization 時に 1 回だけ保存し delegation を seal）、委任（execution lifetime）、検証済み Action 試行相関（authoritative set。seal 時点で固定。result-local）、Task-wide completion barrier（同じ `TaskId` に属する全 revision / 全 delegation の Action 試行に `Unknown` が無いこと。result-local 相関とは別概念）、Task progress（非 terminal の admission gate と terminal（`Cancelled` を含む）の識別）、execution seal（AU14/AU5 の not-sealed gate を含む）、中断対象の `TaskId` と source（会話 Owner message ID / accepted sequence または認証済み管理操作 source）、会話 source の role・Companion・Task 対応 / 直接的中断意図と同じ Task への後続の明示撤回・置換を同一 commit 内で照合、受理（`cancelled` CAS commit）＝停止完了ではないこと。会話のログ記録とタスク反映内容の区別。 |
| **アクション要求・試行・停止・結果** | アクション試行参照（試行ID、タスクリビジョン前提、委任スコープ、操作対象と種別、依拠した権限評価）、`Create / Edit` の実書込本文と既知 source lineage（durable attempt には本文・hash を複製しない）、`Read / List` の対象と取得データの最終利用条件、対象解決の前提、AU5 と作用直前・結果再保存の current erasure condition / provenance cut、`DataUseHeld` と started attempt の区別、コスト・停止・保留・消去・復元の各条件、確認された作用と確定度（成否不明を含む）、リトライ時の元試行参照（`prior_attempt`）。 |
| **権限判断の依頼と回答** | 判断対象（主体、委任、タスク、目的、操作対象、送信先、コストリスク）、依拠したルールと同意のリビジョン、端末条件、重要な状況変化の有無。過去の判断ログと現在の生きた許可の区別。 |
| **プロバイダ送信（初回・フォールバック・再送・継続）** | 解決された割り当て経路、送信先、利用目的、コスト枠の同意対応、認証情報の用途制限、予約済みの利用枠ID（`usage_id`）、送信する論理入力の canonical source 相関（`data_use`）と現在の消去条件による coverage の有無（already-started use としての durable な attempt 相関を含む）、報告済み/不明/処理中の区分。 |
| **クライアント依存活動の開始・継続** | 滞在情報（コンパニオンID、状態、アクティブ端末、世代番号）、端末が申告する世代番号、会話ラウンド、新旧・移行中・アクティブなし・停止中の区分、実際の接続状況。 |
| **画面監視（Observer）のルーティング** | ルーティングコンテキスト（情報源、対象、目的、制約）、取得元端末、取得日時、滞在世代番号、消去・失効条件。 |
| **長期記憶・要約・根拠** | 根拠参照、記憶IDとリビジョン、元データの範囲と取得日時、共有スコープと制約、訂正と状況変化の区別、消去条件。 |
| **消去・保持・バックアップ・復元** | 削除操作IDと消去条件（目的、範囲、対象、保留状態、検索トークンの破棄状況）、バックアップ時点と参照関係、復元世代番号、復元後の保留状態、監査ログの保持。 |
| **利用量・コスト** | 用途と送信先の対応、報告済み/成否不明/処理中の区分、上限枠の現在条件、仮押さえした予約ID（`usage_id`）。 |
| **キャラクター定義の適用** | `(character_id, 期待されるcharacter_revision)`、ユーザーの選択参照、適用されるパーツ群。 |

- **判定結果の表現**: 確定処理（commit）の結果は、少なくとも `Accepted`（受理）、`StalePremise`（前提食い違い）、`HoldActive`（保留中）、`Denied`（拒絶）、`NeedsReevaluation`（再評価が必要）、`RecordedToOriginalOnly`（遅延結果として元履歴にのみ記録）を明確に区別して返します。これは並行比較（concurrency compare）の結果判定であり、各ドメインのライフサイクル状態（Task 状態、滞在状態、確定度、システム全域の操作状態など）を 1 つに潰した共通 `Status` enum ではありません。共通のライフサイクル状態マシンを新設してはなりません（[CI §2.1](correspondence-identity.md#21-ownership-を奪わない責任境界の維持)）。これらは一般的な分類語であり、ドメインごとの具体的な型バリアントは各機能オーナーのインターフェースが定めます（例えばタスクの方向転換は [IB §13.2](interface-boundaries.md#132-domain-別の-repository-interfacepseudo-trait必要なものだけ-abstract-する) の `TaskCommitOutcome`）。
- **シリアライズ境界での厳格なフィールド化**: 直列化の境界では ID、リビジョン、世代番号、関連付け情報を明示的なフィールドとして扱い、自由記述の本文テキストに含まれる文字列を同一性の照合に使ってはなりません（[CI §4.6](correspondence-identity.md#46-シリアライズ境界での基本ルール)）。

## 17. Rust での実装指針（Concrete Rust Implications）

本書の並行特性と永続化設計から導かれる**実装方針の目安**を以下に示します。過去の古い実装の型名や構造を無理に引き継ぐ必要はありません。要件と設計を満たす最小限で簡潔な機構を選択します。

### 17.1 `tokio::sync` プリミティブの使い分け

| 用途 | 推奨されるプリミティブ | 避けるべきアンチパターン |
|---|---|---|
| **直列化ドメインの順序制御** | `mpsc` チャンネル＋局所的なワーカタスク、または短い `Mutex`（タスクごと、コンパニオンごとの狭いスコープ） | すべてを統括する巨大な単一アクター、全ドメイン共通の巨大メールボックス。受信ループ内での長時間の待機（await）や他ドメインのロック待ち。 |
| **1回限りの応答要求** | `oneshot` チャンネル | 受付応答を「処理の完了」と誤認すること（受付と完了は別個に扱います）。 |
| **画面表示・イベント通知** | `broadcast` チャンネル（通知の遅延やスキップが許容される用途） | イベント通知を権限判定やコミット順序、処理完了の証拠として利用すること。 |
| **協調的キャンセル** | `Notify` や標準的な cancellation token。`select!` と `timeout` による中断 | 非同期タスクの drop だけで停止完了とみなすこと。キャンセル時にAIの正常終了をいつまでも待つこと。 |
| **短い排他制御** | `tokio::sync::Mutex` はごく短い順序制御に限定。同期DB処理は `spawn_blocking` 等で非同期実行部から適切に隔離 | ミューテックスを `.await` を跨いで長時間保持すること。同期DB接続を複数の非同期タスクで不用意に共有すること。 |
| **保留・上限変更の通知** | `Notify`、必要であれば `watch` チャンネル | 負荷の高いビジーループによる待機ポーリング。`watch` で読んだ値を恒久的な権限とみなすこと。 |

### 17.2 局所的なワーカタスク / アクターの設計

- タスクごとの受付、コンパニオンごとの滞在管理、削除処理の完了集約、コスト枠の仮押さえなどは、局所的なチャンネルや短いミューテックスで順序を制御します。
- 重いAI推論、外部ツールの呼び出し、ステージング検証などは、直列化の外側の非同期タスクとして並行に実行し、直列化側は前提の確認と結果の安全な受け入れのみを担います。
- システム終了（シャットダウン）時は、「新規受付の停止 → 実行中処理の協調停止 → 完了待ち → 未完了状態の安全な永続化」という段階を踏んで安全に終了させます。

### 17.3 トランザクションクロージャと比較更新スタイルの実装例

同期的な SQLite 接続（`rusqlite` 等）を `spawn_blocking` 経由で使用する場合の概念実装例です（※解説用の疑似コードであり、そのままコンパイルするためのものではありません）：

```rust
/// ミリ秒単位の短い Immediate トランザクションを実行するヘルパー。
/// トランザクションの内部で非同期の await を行ってはならない。
fn with_immediate_tx<T>(conn: &Connection, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let out = f(&tx)?;
    tx.commit()?;
    Ok(out)
}

/// タスク方針変更（steering）の比較更新（CAS）の例。過去のリビジョンも履歴として残す。
/// コンテキスト項目の識別子（entry identity）は前提構造体が保持する。
fn cas_task_steer(conn: &Connection, premise: TaskCommitPremise) -> Result<TaskCommitOutcome> {
    with_immediate_tx(conn, |tx| {
        let Some(current) = select_task_current(tx, premise.expected.task)? else {
            return Ok(TaskCommitOutcome::MissingTask { task: premise.expected.task });
        };
        // terminal は吸収的であり、revision stale とは区別して書き込みなしで拒否する（AU4 の terminal gate）
        if current.progress.is_terminal() {
            return Ok(TaskCommitOutcome::TaskTerminal {
                task: premise.expected.task,
                progress: current.progress,
            });
        }
        if current.revision != premise.expected.revision {
            return Ok(TaskCommitOutcome::StaleExpected { current: current.reference });
        }
        let Some(next) = current.revision.checked_next().filter(is_representable) else {
            return Ok(TaskCommitOutcome::RevisionExhausted { task: premise.expected.task });
        };
        insert_task_revision(tx, &premise, &next)?;      // 新目的または直前値・担当
        update_task_current(tx, &premise, &next)?;       // 現在 purpose・目的本文（None は直前値）
        insert_task_context_entry(tx, premise.adopted_purpose_entry, &next)?; // reference は repository が刻む
        if let Some(instruction) = &premise.adopted_instruction {
            insert_task_context_entry(tx, instruction.entry, &next)?; // 同じ tx。由来・取得時点は premise が持つ
        }
        Ok(TaskCommitOutcome::CommittedAs(TaskRef {
            task: premise.expected.task,
            revision: next,
        }))
    })
}

/// コスト上限枠の仮押さえ（reservation）の例。
/// 予約レコードの登録と上限チェックを同一トランザクションで不可分に行う。
fn reserve_usage(
    conn: &Connection,
    cap: CapId,
    upper_bound: UsageAmount,
    attr: UsageAttributionRef,
) -> Result<ReserveOutcome> {
    with_immediate_tx(conn, |tx| {
        let id = insert_usage_reserved(tx, &cap, &upper_bound, &attr)?;
        // Reserved + Committed + Unknown の合計値を計算（解放済みの Released は除外）
        let total = sum_usage(tx, &cap)?;
        let limit = load_cap_limit(tx, &cap)?;
        if total > limit || must_hold_for_unknown(tx, &cap)? {
            // 上限を超える、または成否不明枠により安全が確認できない場合は枠を解放して拒絶
            mark_released(tx, &id)?;
            return Ok(ReserveOutcome::DeniedOrHold);
        }
        Ok(ReserveOutcome::Reserved(id))
    })
}
```

- **ドメイン固有の結果型の維持**: ドメインごとの `*Outcome` 型は並行比較（concurrency compare）の結果判定であり、各ドメインのライフサイクル状態（`Status`）ではありません（第16節）。共通の単一 enum へ安易に潰してはなりません。
- **エラー型の設計**: ライブラリのエラー定義には `thiserror` を使用し、安易な `String` や `Box<dyn Error>` を公開エラー型として使いません。
- **安全性の確保**: `unsafe` ブロックを使用する場合は、健全性を証明する `// SAFETY:` コメントを必ず付与します（ただし本書の設計のために `unsafe` を要求することはありません）。

## 18. 競合シナリオの検証（Validation / Race Walkthrough）

システムの堅牢性を確認するために、想定される典型的な競合シナリオと、それに対する本設計での安全な動作を検証します。

| 競合シナリオ | 動作の流れと安全性の担保 | 本書での成立箇所 |
|---|---|---|
| **会話の推論結果 vs ユーザーからの新しい追加指示** | 推論開始時の前提（タスクリビジョン、目的、会話ラウンド等）を保持し、推論結果が届いた際に最新状態と突き合わせます。ユーザーからの新しい指示が先に確定していた場合、古い回答は現在の会話には採用せず、過去の思考ログとしてのみ記録します。現在の状況に合致した結果のみを画面に表示します。 | 第5節, 第7節 |
| **採用指示本文の解決 vs steering** | R2 の current context と採用指示本文（`origin.source` の History 行）を claim の外で bounded read（`message_id` PK の単一メッセージ）し、論理入力を 1 回 scrub した後に steering が R3 を確定した場合、AU14 claim が依拠 TaskRef の不一致を同一トランザクションで検出して provider 送信前に拒否します（作業側は再読込で `StaleTaskRevision { current: R3 }` へ写像）。本文 read は長い transaction や Task-wide mutex を追加せず、指示本文の欠如は provider I/O なしの domain outcome（`InstructionSourceMissing`）として扱い、別 Companion・Owner 以外の role・source 不一致は技術的エラーとして fail closed します。 | 第5節, 第7節 |
| **採用指示本文の利用 vs Targeted Deletion（read → deletion → send admission）** | 採用指示本文を bounded read・scrub した後、provider 送信 admission（AU14 claim）の前に deletion が開始され消去条件が durable になった場合、claim は `data_use`（採用目的・全採用指示の `origin.source`）の coverage を同一不分区間で検出し、`DataUseHeld` として送信を拒否します（provider へ 0 バイト。bounded read 時に存在した source を `InstructionSourceMissing` へ偽装せず、`StaleTaskRevision` / storage エラーへも丸めません）。claim 前に materialize 済みの本文があっても、claim と消去条件の durable 化が同一 master で直列化されるため、この 2 順序以外の隙間はありません（V-11 Case 11・13）。 | 第5節, 第7節, 第11節 |
| **provider 送信確定 vs Targeted Deletion（admission → deletion）** | source-aware な送信 admission が先に commit し、attempt 行（`data_use` 相関つき）が durable になった後で deletion が開始された場合、attempt は already-started use として削除参加に列挙され、停止・縮小と遅延結果の再保存防止の対象になります。deletion は確定済み attempt を「未送信」へ書き換えず、遅延 provider 結果は受入境界で現在の消去条件と照合され、削除対象本文を現在 Task へ採用・再保存しません（確定度も改ざんしません。V-11 Case 12）。 | 第5節, 第7節, 第11節, 第14節 |
| **Workspace `Create / Edit` 本文 vs Targeted Deletion** | deletion condition commit が先 → AU5 は実書込本文の mechanical coverage・既知 source / 対象 coverage を同一 SQLite master の `Immediate` で検出し `DataUseHeld`（試行 0・作用 0）。AU5 commit が先 → body-free source / cut 順序を持つ attempt は削除参加の already-started fact として残り、作用直前に再照合して covered / 判定不能なら止め、結果本文も保存時に再照合する。`Read / List` も対象・source の AU5 gate と取得データの利用前 gate を通す。旧 attempt の確定度は停止要求だけでは変更しない。 | 第8節, 第11節, 第15節 |
| **Stage 4 foundation と Stage 6 deletion producer の接続（no active condition → later activation）** | Stage 4 erasure-currentness foundation の canonical current-condition store（active set 0 件の authoritative な照会）× 後から Stage 6 producer が durable-before-enforce で投入する active covering condition × AU14 claim の同一不分区間の coverage 照合 | active condition が 0 件でも store への実照会で被覆なしを確定して送信を enable し、`NoDeletion` sentinel を使いません。Stage 6 は Stage 4 の gate を置換せず、同じ store を拡張します（V-11 Case 14・15）。 | 第5節, 第7節, 第11節 |
| **会話起点 cancel vs 新 Owner 入力 / 管理 cancel** | 会話起点 `CancelTaskCommand` は `OwnerTaskCancelIntentRef` を渡し、会話担当が認証済み Owner 入力と不可分に記録した body-free intent の PK から Owner message ID・accepted sequence・Owner role・Companion・対象 Task・直接的中断意図を AU16 の progress CAS と同じ SQLite master の `Immediate` トランザクションで検証します。元 intent / Task に結ぶ後続の型付き fact は索引付き限定範囲で読み、History 全文走査・LLM 推測・最新 accepted Owner message 比較をしません。後続の認証済み Owner 指示が同じ Task の中断を明示撤回・置換した場合だけ `Superseded`（書き込み・停止通知なし）、曖昧なら `NeedsClarification`（受付保留）。「止めて」の後の「状況も教えて」や別 Task の発話は撤回ではありません。AU16 が先なら `Cancelled` が残り、後の入力は巻き戻しません。管理経路は認証済み source / 対象 Task を照合し、会話 currentness に依存せず同じ progress 比較と CAS を使います。steering と cancel は同じ master 上で直列化され、revision・purpose の不一致だけでは cancel を拒否しません。 | 第5節, 第7節, 第14節 |
| **schedule 到来 vs Stop / Deletion / Resume / restart** | 同じ回は一意キーで一度だけ確定。予定 UTC 時刻が証明済み Stop / Host 停止区間内なら `Missed`・Task なし、Stop より前の未受付回は `Unclassified`・Task なし、到来の commit が先なら `Started`・Task を残します。分類済み安全境界の後に稼働中の due を受付前に crash した場合、停止の durable 下端がないため startup 固定上端までの不確実区間を `Unclassified`・Task なしとして Owner に通知します。別 epoch の停止区間は `Missed`、現 epoch の遅延 due は bounded に受付、未来回は先読みしません。版切替と区間の交差は旧 snapshot から処理し、cursor は schedule / 版 / 区間ごとに独立して crash 後再取得します。削除 hold・restore hold を解除せず、Run now は別 Task です。 | 第5節, 第10節、PR §6・§7 AU18 |
| **エージェントの作業結果 vs 追加指示・キャンセル** | final result の到着はまず安全な本文または body-free held の identity を 1 回だけ durable record し、同じ不分区間で delegation を seal し（1 delegation につき final result は最大 1 つ）、採用判定で依拠リビジョン（delegation 行から解決）と現在のタスクリビジョン・purpose identity を 1 つの短いトランザクションで比較します。食い違っている場合は現在の成果としては採用せず、検証済み attempt 相関のみを記録して `RecordedToOriginalOnly` とします。delegation（execution lifetime）から列挙した authoritative set と claim が一致しない場合は技術的エラー（fail closed）、result-local の `Unknown` / 非依拠が検証できない失敗があるか、または同じ `TaskId` の全 revision / 全 delegation の Action 試行に `Unknown` が残れば（Task-wide completion barrier。blockers は両者の和集合として重複を 1 回に畳む）、`WithheldByEffectFacts` として完了しません（seal 後の証拠更新で再評価可能）。AU5 と AU15b は同一 SQLite master 上の短い `Immediate` トランザクションで直列化されるため、開始済み Action が barrier に見えず、かつ terminal gate も通る順序は存在しません（`AU5 → barrier sees Unknown → Withheld` か `Completed → AU5 TaskTerminal` のいずれかです）。seal 後は同じ execution からの新規 inference claim / Action 開始を拒否し、中断（AU16。`task.progress` の `cancelled` への CAS commit）や terminal 後は後続処理を自動開始せず、未完了・成否不明な状態を正直に報告します。 | 第5節, 第7節, 第14節 |
| **並行して実行される2つのアクション試行** | 開始時の前提確認とコスト枠の仮押さえを短いトランザクションで直列化し、アクションの実行そのものは並行して行います。重複の危険がある再試行は、ユーザーへの確認と元試行への関連付けを必須とします。成否の確定は試行ごとのアトミックな更新で独立して行います。 | 第8節 |
| **アクションのタイムアウト vs 遅れて届いた成功応答** | タイムアウトしても結果の確定度は `Unknown`（成否不明）として維持します。遅れて成功通知が届いた場合、元の試行レコードを `Unknown → ConfirmedSuccess` とアトミックに更新します。現在のタスクへ採用するかは、現在の状況と照らし合わせて別途判断します。 | 第8節, 第14節 |
| **コスト上限付近での複数の並行推論リクエスト** | すべてのリクエストは `reserve_usage` による同一トランザクションでの事前仮押さえを行います。先に受け付けられたリクエストで上限に達した場合、後続のリクエストは安全に保留・拒絶され、上限突破を防ぎます。処理中や成否不明な枠を勝手にゼロとみなすことはありません。 | 第9節 |
| **端末移動（AからBへ呼び出し） vs ネットワーク切断** | 両者を `SD-Presence` の順序で直列化し、世代番号の比較更新により単一の滞在先のみを成立させます。移行期間中は新旧どちらの端末でも新規処理を開始しません。古い端末の未完了処理を別の端末で勝手に自動継続させることはありません。 | 第10節 |
| **古い端末からの遅延再接続 vs 復元された滞在状態** | 端末が申告する世代番号、ホスト側の最新世代番号、実際の接続状況、最新の権限を厳格に照合します。古い一時キャッシュや過去の承認情報だけを頼りに接続を成立させることはなく、確認が取れない場合は処理を安全にブロックします。 | 第10節 |
| **新しい記憶の形成 vs 個人データの完全削除** | 記憶をDBに保存する際、最新の制約に加えて進行中の消去条件を照合します。削除対象区間に含まれるデータはすべて消去対象とし、実行中の処理が誤って再保存することを防ぎます。検証が終わる前に完了と表示することはありません。 | 第11節 |
| **完全削除の完了 vs 遅れて届いた結果 / クラッシュ** | 各機能の削除集約、残存検証、再到着データの処理を終えた後、検索用トークンを完全に復元不能化し、その成功を確認してから全域完了を確定します。トークン破棄前にクラッシュした場合は未完了（`finalizing`）から再開します。完了後に届いた過去の関連データは再保存を阻止します。 | 第11節 |
| **バックアップ復元の切替 vs 復元前の旧実行結果** | ステージング領域での検証を経て、復元世代番号のアトミックな切り替えを行います。切り替え前は旧データ、切り替え後は新データがマスターデータとなり、混ざり合いません。復元前の古いセッションや古い同意情報だけで処理が勝手に動き出すことを防ぎます。 | 第12節 |
| **restore staging vs deletion start / completion** | 旧マスターでの新しい削除開始・sweep 前進・finalizing / completion が準備後に確定したら、最終比較は stale として切替を止め、長い再準備を境界外で行います。比較が先なら切替と現行条件・hold・cut / 材料参照が一体で公開され、後続 deletion commit は新マスターにだけ行います。復元で戻る対象は新 sweep に含め、旧 participant の `Verified` は流用しません。材料や条件の証明ができなければ対象範囲を hold して延期します。 | 第12節, 第15節, PR §9 |
| **定期クリーンアップ vs ファイル保存処理** | ファイルをリネームしてからDBポインタを登録するまでの間、公開保護（publication guard）によって定期クリーンアップによる誤削除を防ぎます。DBポインタの登録が完了した後はDBからの参照によって保護されます。クラッシュ等で保護もポインタも残らなかった場合のみ不要ファイルとして掃除されます。 | 第13節, 第15節 |
| **ファイル保存とDB登録の境目でのホストクラッシュ** | 「ファイル永続化 → 公開保護 → DBポインタ登録 → 可視化」の順序を守るため、途中でクラッシュしても中途半端なデータが正常データとして画面に表示されることはありません。 | 第13節 |
| **連続する追加指示 vs 委任エージェントの作成** | タスクごとの直列化により指示の適用を順番に処理し、古い指示を前提とした委任要求は安全に不受理として最新指示のもとで再評価させます。 | 第7節 |
| **処理実行中のユーザーによる権限失効** | 以後の新規リクエストを即座に拒絶し、実行中の処理に対して可能な限り停止を伝えます。すでに確定した過去の消費記録を勝手に書き換えることはありません。 | 第9節, 第14節 |
| **キャラクター定義の更新 vs 適用処理の競合** | 新しいキャラクター定義の追加と個体への適用処理を分離し、適用は期待リビジョンとユーザー選択の比較更新によってアトミックに確定します。未確認のパーツが中途半端に適用されることを防ぎます。 | 第4節, 第5節 |

## 19. 意図的に残した設計の自由度（Design Freedom）

本設計ではシステムの信頼性とセキュリティに関わる重要な境界を厳格に固定する一方で、実装の詳細については将来の改善や柔軟な選択ができるよう、意図的に自由度を残しています。

| 設計対象 | 厳格に固定したアーキテクチャ原則 | 実装者に委ねる自由度（Design Freedom） |
|---|---|---|
| **保存・トランザクション実装** | 永続化と可視化の順序、確定直前のアトミック照合、ファイル保存とクリーンアップの同期排他ルール。 | 具体的な SQL 文、テーブルインデックス、マイグレーション手順、バキューム設定、使用するトランザクションライブラリ、ファイルのディレクトリ構造、fsync API の具体的な呼び出し方、クリーンアップの実行周期。 |
| **並行処理メカニズムの実装** | 直列化ドメインの境界、ドメインごとのメカニズム選定、ロックの取得順序と await 中の保持禁止ルール。確認が取れない状態を勝手に許可や完了に変換しないこと。 | ワーカタスクやメールボックスの具体的なクレート配置、キューの種類、Mutex や RwLock の最終選択、キャンセルトークンの具体的な選定、プロセス間通信の形式、リトライやタイムアウトの数値。 |
| **利用コスト・リソース枠** | 同一の残枠を複数リクエストが同時に奪い合って上限を突破することを禁止すること。処理中や成否不明な枠を勝手にゼロとみなさないこと。 | 仮押さえ枠の具体的な計算式、集計期間、推定方式、上限値の設定、管理画面での表示粒度。 |
| **派生データ・要約・ルーティング** | 派生データを勝手な第2のマスターにしないこと。無効化キー（リビジョン、世代、消去条件）を保持し、データ削除処理に必ず参加させること。 | 埋め込みベクトルの計算方法、類似度スコアリング、キャッシュの実装方式、要約の生成タイミングやプロンプト形式、インデックス再構築の頻度。 |
| **システム全域操作の運用** | 単一マスターの原則、中途半端な混合状態の禁止、権限の勝手な先行復活の禁止、成功表示の厳格な条件、再起動時の安全な保全。 | 停止シグナルの伝達方法、切断検知のタイムアウト時間、滞在切り替えの調停手順、バックアップの整合時点の作り方、部分的な失敗からの復旧手順。 |
| **通信疎通・時刻・UI** | 経過時間の解釈、失われた通信の勝手な補完の禁止、確認不能な状態を安全側に倒して不受理とすること。 | 疎通確認の具体的な通信プロトコル、時間の減衰計算、画面キャプチャの取得タイミング、ユーザーへの提示確認方法、要約の粒度、UIの画面レイアウト。 |

アーカイブ形式、暗号化の実装、シリアライズ形式、Rust の具体的な型やトレイトの定義、クレート分割の詳細も固定しません。これらはホストPCとクライアント端末の信頼境界、および各機能の責任分担を遵守した上で、最もシンプルで信頼性の高い実装手法を選択します。
