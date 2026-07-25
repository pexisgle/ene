# `ene-plugin` — APIリファレンス

> **クレート:** `ene-plugin`
> **役割:** プラグイン作成ファサード（API v1）。新しいツール/プラグインバイナリの推奨 import パス。
> **依存しないもの:** `ene-runtime`、`ene-mind`、`ene-store`。

---

## 概要

`ene-plugin` はツールバイナリが使うワイヤー層・ホストアダプタ層・derive 層を再エクスポートします。

| 層 | 型 | 元クレート |
|---|---|---|
| ワイヤー（IPC / サンドボックス） | `ToolProvider`、`IpcRequest` / `IpcResponse`、`ToolSpec` | `ene-plugin-proto` |
| サーバー / レジストリ | `run_tool_server`、`HostRegistry` | `ene-plugin`（このクレート） |
| ホストアダプタ | `ActionSetProvider`、`SingleActionProvider`、`ToolAction` | `ene-tool-common` |
| Derive | `#[derive(ToolSpec)]`、`#[derive(ToolAction)]` | `ene-tool-derive` |

ホスト集約（`ToolRegistry`、MCP、複合ツール、Tool RAG）は [`ene-plugin-host`](ene-plugin-host.md) に残ります。リーフクレートをこのファサードへ物理統合するのは follow-up です。それまではリーフはワークスペースに残り、本ファサードがサポートされる import パスです。

## 二層契約

- **ワイヤー:** ツールバイナリは `ToolProvider` を実装し、handshake / list / call / permission・user-input 継続 / shutdown を話す。
- **ホスト:** `ene-plugin-host::ToolRegistry` が IPC + MCP を集約する。
- **名前衝突:** 全レジストリ層でハードエラー（`HostRegistry` は `ToolError::DuplicateName`；`CompositeToolRegistry` は `ToolHostError::DuplicateToolName` を返す）。
- **`ToolSpec`** は LLM 向け: `name`、`description`、`parameters`。
- **`ToolRagProfile`** はホスト/RAG 専用 (#137): キーワード、例、カテゴリなど — LLM のツールリストには渡されない。

```rust,ignore
use ene_tool_common::prelude::*;
```

## 関連

- [`ene-plugin-proto`](ene-plugin-proto.md) — IPC ワイヤープロトコル
- [`ene-tool-common`](ene-tool-common.md) — `ToolAction` ヘルパー
- [`ene-tool-derive`](ene-tool-derive.md) — derive マクロ
- [`ene-plugin-host`](ene-plugin-host.md) — プロセス管理と Tool RAG
- [ツールシステム概要](../tools/overview.md)
- [API v1](../architecture/api-v1.md)
