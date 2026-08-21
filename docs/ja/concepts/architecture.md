# アーキテクチャ

Ene は **コンパニオン型ハーネス**です。コアデーモンは1プロセス、クライアントは複数、
ツールはアウトプロセス、認知層はホスト内です。

完成形の定義は
[`plans/harness-redesign/`](../../../plans/harness-redesign/README.md) にあります。
このページは、今のツリーにあるコードの説明です。

## プロセスモデル

```text
ene-stage   ─┐
ene-desktop ─┤
ene-ctl     ─┼── HTTP/WS (ene-api) ──► ene-core (ene-daemon)
Web         ─┘                              │
                                            ├── ene-session / ene-kernel
                                            ├── ene-companion / ene-body / ene-work
                                            ├── ene-plane (承認 + 監査 + ボールト)
                                            └── ene-fiber ──► plugins/tool/*
```

- **ホストは1つ。** 本体の状態は `ene-core` が持ちます。クライアントはカーネルを埋め込みません。
- **クライアントは対等**です。排他資源（マイク、承認応答）だけデーモンが調停します。
  製品 GUI は `ene-stage`、`ene-desktop` は同一 API の凍結レガシーで、
  stage が代替できたと判断したら削除します。
  [製品境界](product-boundaries.md) を見てください。
- **ツールはアウトプロセス。** ビルトイン (`fs` / `exec` / `web` / `utility` /
  `app`) もサードパーティと同じ IPC です。コンパニオン状態に触るハーネス機能は
  ホスト内で、`ene-registry` を通します。

## 2層、1体のコンパニオン

各コンパニオンは **表層 soul**（対話レーン）と **裏層ハーネス**（ジョブ・委譲・
スケジュール）を持ちます。ユーザーが話す相手は表層だけです。複雑な作業は委譲し、
ジョブレーンがモデルとツールを回し、進捗はコンパニオンの発話として返ります。

表示の深さは `surface` か `detail` です。何を送るかはサーバが決めます。
stage のキャラクターとチャットは surface、別窓の詳細は detail です。
旧 desktop も同じ深さを使います。

## 次に読むもの

| 話題 | 文書 |
|---|---|
| クレート地図と依存規則 | [クレートリファレンス](../reference/crates.md) |
| どのクライアントが製品 GUI か | [製品境界](product-boundaries.md) |
| プラグイン IPC | `ene-plugin-ipc` の rustdoc |
| キャラクターパッケージ | [キャラクターパッケージ](character-cards.md) |
| 設計決定 | [`plans/harness-redesign/decisions.md`](../../../plans/harness-redesign/decisions.md) |
