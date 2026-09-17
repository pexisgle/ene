# Stage 2: セットアップとテキスト会話の最小開通

[実装ガイド](../README.md) / [進捗](../PROGRESS.md)

この文書は Stage 2 の**実装順と完了条件**だけを扱います。製品要件は `docs/requirements/`、内部設計は `docs/design/` を優先します。

## 目的

初期設定から一対一のテキスト会話までを、Host・Client・推論・永続化を含む最初の縦断スライスとして通します。

## 実装順

1. Host と Client の接続および最小ハンドシェイクを開通する。
2. 言語選択と API key 登録を含む最小セットアップ経路を作る。
3. 権限・認証情報・推論の境界を接続する。
4. OpenAI Responses API を使う provider transport と推論パイプラインを接続する。
5. Client からのメッセージ入力と AI 応答のストリーミング表示を通す。
6. 会話履歴を durable に保存し、Host 再起動後にタイムラインを復元する。
7. setup → input → inference → stream → history → restart を一つの integration test で通す。

各層を長期間別々に作り込まず、まずこの最小経路が end-to-end で動くことを優先します。

## 完了条件

- Client からメッセージを送り、パートナーの応答を表示できる。
- 保存済み会話が再起動後も復元される。
- Linux / Windows の双方で Stage 2 の縦断経路が再現できる。

## 非対象

Experience / Memory、Task Agent、Client lifecycle recovery などは後続 Stage で追加します。
