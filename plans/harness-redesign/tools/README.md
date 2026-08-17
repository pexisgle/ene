# tools/ — ツールと capability

モデルが「手」を使うための2文書。

この層に効く決定は3つある。

1. **実行場所とモデルからの可視性は別の軸**(D-10)。「全部アウトプロセス」
   ではなく、ハーネス状態と密結合するもの(記憶想起・委譲)はホスト内で
   動く**ハーネス機能ツール**になる。それ以外のツールはビルトインを含めて
   アウトプロセス。
2. **表層に公開するかは `side_effects` が空かどうかで決まる**(D-2)。
   「簡単さ」では決めない。
3. **内製は5つに絞り、残りは MCP に委ねる**(D-23)。
   内製は `app` / `fs` / `exec` / `web` / `utility`。
   `fs.undo` は当該 job が書いたものだけを戻す(D-30)。
   MCP の接続導線 UI は後継(P-616)。

| 文書 | 責務 | 主な P 対応 |
|---|---|---|
| [registry.md](registry.md) | 統一ツールレジストリ・2軸の分類・ToolDefinition・実行パイプライン・出力正規化・層ごとの公開面 | P-601, P-613, P-614, P-615 |
| [capabilities.md](capabilities.md) | capability seam 群(fs/exec/web/app/utility/code/provider/vision)+ MCP 接続 + 補助LLM | P-602..P-604, P-611, P-612, P-112, P-524, P-616, P-1003 |

## 他フォルダとの接点

- 表示の深さ → [../core/visibility.md](../core/visibility.md)
- 委譲への自動昇格 → [../core/agent-loop.md](../core/agent-loop.md)
- 実行パイプラインの承認 → [../security/approval.md](../security/approval.md)
- 資源の仲介 → [../plugins/broker.md](../plugins/broker.md)
- spill/出力上限 → [../core/context-assembly.md](../core/context-assembly.md)
- skill の注入 → [../tasks/skills.md](../tasks/skills.md)
