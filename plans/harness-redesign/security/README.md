# security/ — 安全とプライバシー

2文書。多層防御の「隔離」と「判断」。

| 文書 | 責務 | 主な P 対応 |
|---|---|---|
| [sandbox.md](sandbox.md) | OS 強制サンドボックス・見える世界の最小化・資源制限・検証 | P-901 |
| [approval.md](approval.md) | 承認 plane(ポリシー/AI/ポップアップ)・監査ログ・資格情報ボールト・プライバシー | P-903..P-908, P-910 |

## 多層防御の全体

```text
ツール実行要求
  → 承認 plane(判断: sandbox.md の外)   [approval.md]
  → Broker(資源の仲介・委譲)              [plugins/broker.md]
  → サンドボックス(进程の隔離・強制)       [sandbox.md]
  → 監査ログ(全操作の記録)                 [approval.md §6]
```

承認が「していいか」、Broker が「どう渡すか」、サンドボックスが
「閉じ込める」、監査が「残す」。4層が揃って初めて安全になる。

## 他フォルダとの接点

- ツールの副作用宣言 → [../tools/registry.md §2](../tools/registry.md)
- 資源委譲の窓口 → [../plugins/broker.md](../plugins/broker.md)
- 記憶の承認キュー → [../companion/memory.md §7](../companion/memory.md)
- ポップアップの配信・応答 → [../platform/clients.md §3](../platform/clients.md)

