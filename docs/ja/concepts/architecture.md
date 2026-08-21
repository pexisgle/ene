# アーキテクチャ

Ene は **コンパニオン型ハーネス**です。コアデーモンは1プロセス、クライアントは複数、
ツールはアウトプロセス、認知層はホスト内です。

完成形の定義は
[`plans/harness-redesign/`](../../plans/harness-redesign/README.md) にあります。
このページは、今のツリーにあるコードの説明です。

## プロセスモデル

```text
ene-stage   ─┐
ene-ctl     ─┼── HTTP/WS (ene-api) ──► ene-core (ene-daemon)
Web         ─┘                              │
                                            ├── ene-session / ene-kernel
                                            ├── ene-companion / ene-body / ene-work
                                            ├── ene-plane (承認 + 監査 + ボールト)
                                            └── ene-fiber ──► plugins/tool/*
```

- **ホストは1つ。** 本体の状態は `ene-core` が持ちます。クライアントはカーネルを埋め込みません。
- **クライアントは対等**です。排他資源（マイク、承認応答）だけデーモンが調停します。
- **ツールはアウトプロセス。** ビルトイン (`fs` / `exec` / `web` / `utility`) も
  サードパーティと同じ IPC です。コンパニオン状態に触るハーネス機能はホスト内で、
  `ene-registry` を通します。

## 2層、1体のコンパニオン

各コンパニオンは **表層 soul**（対話レーン）と **裏層ハーネス**（ジョブ・委譲・
スケジュール）を持ちます。ユーザーが話す相手は表層だけです。複雑な作業は委譲し、
ジョブレーンがモデルとツールを回し、進捗はコンパニオンの発話として返ります。

表示の深さは `surface` か `detail` です。何を送るかはサーバが決めます。
stage のキャラクターとチャットは surface、別窓の詳細画面は detail です。

## 次に読むもの

| 話題 | 文書 |
|---|---|
| クレート地図と依存規則 | [クレートリファレンス](../reference/crates.md) |
| プラグイン IPC | `ene-plugin-ipc` の rustdoc |
| キャラクターパッケージ | [キャラクターパッケージ](character-cards.md) |
| 設計決定 | [`plans/harness-redesign/decisions.md`](../../plans/harness-redesign/decisions.md) |
