# アーキテクチャ（現在実装）

> このページは**現在ツリーに存在するコード**を説明します。製品要件ではありません。あるべき挙動は [要件定義](../requirements/README.md) で確定します。

現在の Ene は、1つのコアプロセス、複数クライアント、アウトプロセスの tool/provider plugin、ホスト内のドメインライブラリで構成されています。

```text
ene-stage   ─┐
ene-desktop ─┤
ene-ctl     ─┼── HTTP/WS (ene-api) ──► ene-core
Web         ─┘                              │
                                            ├── ene-session / ene-kernel
                                            ├── ene-companion / ene-body / ene-work
                                            ├── ene-access-control
                                            ├── ene-tool-registry
                                            └── ene-plugin-host ──► plugins/*
```

- `ene-core`: プロセス全体の状態と HTTP/WS。
- `ene-api`: client ↔ core の契約。
- `ene-session`: 追記専用会話ログと usage 台帳。
- `ene-kernel`: 対話レーン。
- `ene-companion`: soul、感情、記憶、内面、能動挙動。
- `ene-work`: delegation、job、schedule、skill、MCP。
- `ene-access-control`: 承認、監査、credential vault。
- `ene-tool-registry`: 統一ツールレジストリ / pipeline。
- `ene-plugin-host`: plugin process の監督と巻き戻し可能な host-side composition。
- tool/provider plugin はアウトプロセスで動作します。

詳細な現在のクレート地図と依存規則は [クレートリファレンス](../reference/crates.md) を参照してください。
