# companion/ — コンパニオン層

「魂」の中身。4文書で**表層 soul**の内面を定義する。
ユーザー入力の唯一の入口であり、作業の遂行は裏層ハーネス
([../core/delegation.md](../core/delegation.md))へ託す。

| 文書 | 責務 | 主な P 対応 |
|---|---|---|
| [soul-and-affect.md](soul-and-affect.md) | soul の永続構造・ハイブリッド感情モデル・関係性・家庭モデル | P-3xx, P-209 |
| [inner-channel.md](inner-channel.md) | 内面イベントの形式・生成・自己参照・UI 表示 | P-104, P-305 |
| [memory.md](memory.md) | ボールト/共有スペース・種別・想起・抽出/裁定/忘却・ユーザー編集 | P-2xx |
| [proactive.md](proactive.md) | 能動発話のゲート→決定→発話・静寂の同居 | P-105, P-106 |

## 他フォルダとの接点

- 2層ランタイムの位置づけ → [../product/vision.md](../product/vision.md#51-コアデーモン内の2層)
- 裏層への委託 → [../core/delegation.md](../core/delegation.md)
- 感情→表情の写像先 → [../body/body-and-performance.md](../body/body-and-performance.md)
- コンテキストへの載せ方 → [../core/context-assembly.md](../core/context-assembly.md)
- スケジュール発火(`TurnOrigin::Scheduled`) → [../tasks/jobs-and-schedules.md](../tasks/jobs-and-schedules.md)(能動発話パイプラインとは別)
- キャラクター定義(ベースライン等) → [../character/package-format.md](../character/package-format.md)

