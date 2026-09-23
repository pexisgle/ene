# Wire・共通型・起動設定の後続計画

[実装ガイド](../README.md) / [Stage 7 A2](../stages/stage-7.md#a2-通常-client-通信の-wss-統一未実装) / [Stage 10](../stages/stage-10.md)

これらは一括の開発停止条件ではない。変更する境界の直前に設計を確定し、独立した整理は並行して進める。古い調査時点の行数・consumer・テスト数は着手時の source とコンパイルで再確認する。

## F1: 通常 wire の unknown 値と round 指定

- [Host–Client IPC §7.2](../../design/concrete/host-client-ipc.md) の negotiated 範囲で、未知 message type / enum value を該当 message の `UnsupportedMessage` / `UnsupportedFieldValue` にし、connection を維持する。現行の closed `WirePayload` 全体 decode は domain ingress 前に失敗するため、discriminator 先読みによる framing か raw catch-all を設計し、codec と Host ingress の両方でテストする。旧版互換シムは追加しない。WSS 化だけで完了としない。
- `SubmitTextInput` の `round: Option<_>` と `fresh: bool` の不正な組合せをなくす。IPC §13.1 で `Auto`（既存 join / 無ければ mint）を正式に採るか、Desktop に `Existing` / `New` の明示選択を求めるか決め、tagged `RoundTarget` と DTO / Host / GUI の意味をそろえる。A2 と Voice 入力接続前に設計・テストを行い、未決の既存挙動を仕様とみなさない。

## F2: `ene-primitive` の共有範囲

- [Crate decomposition](../../design/concrete/crate-module-decomposition.md) の責務表・§4.1・依存表と rustdoc を、現行 consumer に合わせる。小さな共有値クレートを維持する案を第一候補とし、単独 consumer の `GenerationInner` は domain の `PresenceGeneration` に checked increment を置く案を検討する。将来 consumer や汎用 counter trait は先取りしない。
- `RevisionInner` は Task / Memory / Consent / CredentialSet / UsageCap と Action / inference の前提情報に使う境界を調べ、維持・撤去を設計で判断する。domain ごとの型は区別する。`RawId` は発行、保存値復元、決定的 device 対応を分け、再利用・対象照合・直列化・SQLite 保存可能範囲の保証主体を明記する。ID を認証証拠にしない。`Money` は共通表現と単位を定め、料金計算・丸め・許否を owner に残す。`WallClockWithTz` の固定 UTC offset と検索向け UTC を区別する。
- 設計更新後に必要な型だけを変更する。Presence 世代の前進・枯渇、Task 等の stale / 保存可能範囲、金額表現、offset と UTC の性質を実際の利用境界で検証する。互換 alias・shim、不要な分割クレートを作らない。

## F3: 未使用依存の削減（F2 の型判断を待たず評価可能）

- 現行 consumer を検索・コンパイルして、不要な primitive の Serde derive / import / 直接依存と client の `ene-primitive` 直接依存を削除する。実利用が見つかれば保存・通信境界と必要形式を示して最小限を残す。消えた独自 Serializer / Deserializer テスト基盤を復元しない。他 package の Serde・UUID feature や wire DTO を一括削除しない。
- lockfile、保存・wire の利用側、focused tests、workspace CI を確認する。client manifest は F4 の config 依存整理と競合しないよう調整する。

## F4: 起動パス・本文言語・表示言語の分離

- `ene-config` を共通の起動パス解決に絞り、CLI 本文の言語タグは `ene-ctl`、GUI 表示言語は一般設定 owner へ分ける案を設計で比較する。起動 JSON / `--config` は実 consumer・要件がなければ廃止を第一候補とし、残す場合は明示ファイル欠落・読込失敗・未知キーの扱いを定義する。汎用設定 manager や新クレートは前提にしない。
- 起動引数・環境・OS 既定値の優先順位、空値・相対パス・非 Unicode・解決不能時を定め、誤入力で別データ領域へ黙って切り替えない。解決だけでディレクトリや設定ファイルを作らない。GUI → Host、Host → GUI、CLI の接続先を一致させ、秘密を汎用環境ツリーへ収集しない。
- 接続前表示言語と接続後の正本・保存・読込を設計に記録する。切替の保存失敗と再起動復元を表示し、言語切替で会話履歴や安全境界を変えない。GUI の日英選択肢を CLI 本文タグの制限に流用しない。
- 影響する `ene-core` / `ene-ctl` / `ene-desktop` / `ene-client`、必要なら presentation / store / API を更新し、不要な `ene-client → ene-config` 依存を削る。Linux / Windows のパス・GUI 経路、focused tests と workspace CI を確認する。初回接続先選択は [D5](desktop.md#d5-起動時の接続先選択設計後remote-実接続は-stage-14) と整合させる。
