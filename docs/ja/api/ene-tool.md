# `ene-tool` — APIリファレンス

> **クレート:** `ene-tool`
> **役割:** ツール ABI 表面のファサード（API v2）。新しいツールバイナリの推奨 import パス。
> **依存しないもの:** `ene-runtime`、`ene-mind`、`ene-store`。

---

## 概要

`ene-tool` はツールバイナリが使うワイヤー層・ホストアダプタ層・derive 層を再エクスポートします。

| 層 | 型 | 元クレート |
|---|---|---|
| ワイヤー（IPC / サンドボックス） | `ToolProvider`、`IpcRequest` / `IpcResponse`、`ToolSpec`、`run_tool_server` | `ene-tool-proto` |
| ホストアダプタ | `ActionSetProvider`、`ToolAction` | `ene-tool-common` |
| Derive | `#[derive(ToolSpec)]`、`#[derive(ToolAction)]` | `ene-tool-derive` |

ホスト集約（`ToolRegistry`、MCP、複合ツール、Tool RAG）は [`ene-tool-host`](ene-tool-host.md) に残ります。リーフクレートをこのファサードへ物理統合するのは follow-up です。それまではリーフはワークスペースに残り、本ファサードがサポートされる import パスです。

## 二層契約

- **ワイヤー:** ツールバイナリは `ToolProvider` を実装し、handshake / list / call / permission・user-input 継続 / shutdown を話す。
- **ホスト:** `ene-tool-host::ToolRegistry` が IPC + MCP を集約する。
- **名前衝突**はレジストリ構築 / 追加時にハードエラー。
- **`ToolSpec`** は LLM 向けのみ: `name`、`description`、`parameters`。

```rust,ignore
use ene_tool::prelude::*;
```

## 関連

- [`ene-tool-proto`](ene-tool-proto.md) — IPC ワイヤープロトコル
- [`ene-tool-common`](ene-tool-common.md) — `ToolAction` ヘルパー
- [`ene-tool-derive`](ene-tool-derive.md) — derive マクロ
- [`ene-tool-host`](ene-tool-host.md) — プロセス管理と Tool RAG
- [ツールシステム概要](../tools/overview.md)
- [API v2](../architecture/api-v2.md)
