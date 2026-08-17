# tools/ — ツールと capability

モデルが「手」を使うための2文書。公開面は層で分かれる
(表層は作業ツールを見ない、[registry.md](registry.md) の層ごとの公開面)。
ビルトインはバンドル済みアウトプロセスプラグインであり、ホスト内関数ではない。

| 文書 | 責務 | 主な P 対応 |
|---|---|---|
| [registry.md](registry.md) | 統一ツールレジストリ・ToolDefinition・実行パイプライン・出力正規化。Tool Calling と MCP の分離前提 | P-601, P-613, P-614 |
| [capabilities.md](capabilities.md) | capability seam 群(fs/exec/web/browser/connector/code/provider/vision)+ MCP 接続 | P-602..P-604, P-611, P-612, P-1003 |

## 他フォルダとの接点

- 実行パイプラインの承認 → [../security/approval.md](../security/approval.md)
- 資源の仲介 → [../plugins/broker.md](../plugins/broker.md)
- spill/出力上限 → [../core/context-assembly.md](../core/context-assembly.md)
- skill の注入 → [../tasks/skills.md](../tasks/skills.md)

