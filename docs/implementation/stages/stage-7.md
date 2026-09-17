# Stage 7: 管理画面・デスクトップアバター・最初の受け入れ検証

[実装ガイド](../README.md) / [進捗](../PROGRESS.md)

この文書は Stage 7 の**実装順と完了条件**だけを扱います。製品要件は `docs/requirements/`、内部設計は `docs/design/` を優先します。

## 目的

Stage 0〜6 の機能を first-party UI と受け入れ検証へ結びつけ、最初の開発 milestone を完了します。

## 実装順

1. 会話、Memory、Task、usage / cost を一覧できる text-first の管理画面を作る。
2. Stage 6 までに存在する owner query / command を UI へ接続し、UI 専用の authoritative snapshot を新設しない。
3. VRM 1.0 の透明 desktop avatar を表示し、移動・resize・待機 / 発話 animation を実装する。
4. avatar process / rendering が停止しても chat と管理画面が利用できる fallback を固定する。
5. `docs/requirements/acceptance.md` の最初の milestone に含まれる全 scenario を first-party 経路から実施する。
6. Windows / Linux の双方で CPU・memory・FPS などの performance test を実施し、要件の基準を満たす。

## 並列化

- 管理画面と avatar rendering は、共有 API が固定されている範囲で並列化できる。
- acceptance / performance harness は UI 実装と並行して整備できるが、未完成 feature を skip して milestone 完了扱いにはしない。

## 完了条件

- first-party UI から会話・Memory・Task・usage / cost を確認できる。
- avatar failure が chat / management plane を停止させない。
- acceptance の全対象 scenario が Windows / Linux で通る。
- performance criteria を両 OS で満たす。
- 最初の開発 milestone を完了として `PROGRESS.md` を次 Stage へ進められる。
