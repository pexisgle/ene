# companion/ — コンパニオン層

「魂」の中身。4文書で**表層 soul**の内面を定義する。
ユーザー入力の唯一の入口であり、複雑な作業は裏層ハーネス
([../core/delegation.md](../core/delegation.md))へ託す。
簡単な作業は表層が自分で片付ける(D-1)。

| 文書 | 責務 | 主な P 対応 |
|---|---|---|
| [soul-and-affect.md](soul-and-affect.md) | soul の永続構造・ハイブリッド感情モデル・関係性・家庭モデル | P-3xx, P-209 |
| [inner-channel.md](inner-channel.md) | 内面イベントの形式・生成・自己参照・表示(既定 OFF) | P-104, P-305 |
| [memory.md](memory.md) | ボールト/共有記憶プール・種別・想起・抽出/裁定/忘却・ユーザー編集 | P-2xx |
| [proactive.md](proactive.md) | 能動発話の観測→ゲート→決定→統合確認→発話・静寂の同居 | P-105, P-106, P-112 |

## この層に効く主な決定

- **能動発話は現行実装を正とする**(D-28)。当初の下書きは時間駆動だったが、
  実装は既にイベント駆動で、決定的ゲート・世界状態の傾向分析・
  二段のモデル確認を持っていた([proactive.md](proactive.md))。
- **記憶は既定で soul ごと、ただしユーザーに関する事実は共有**(D-7)。
  スコープは記憶抽出の補助LLM が抽出時に判定する。
- **内面の表示は既定 OFF**(D-20)。生成と自己参照は続く。
- **感情は常に body の emotion と連携**(D-19)。内部状態は詳細画面のみ。

## 他フォルダとの接点

- 2層ランタイムの位置づけ → [../product/vision.md](../product/vision.md#51-コアデーモン内の2層)
- 裏層への委託と自動昇格 → [../core/delegation.md](../core/delegation.md)
- 感情→表情の写像先 → [../body/body-and-performance.md](../body/body-and-performance.md)
- コンテキストへの載せ方 → [../core/context-assembly.md](../core/context-assembly.md)
- 表示の深さ(表層UI / 詳細画面) → [../core/visibility.md](../core/visibility.md)
- 補助LLM の位置づけ → [../tools/capabilities.md](../tools/capabilities.md)
- スケジュール発火(`TurnOrigin::Scheduled`) → [../tasks/jobs-and-schedules.md](../tasks/jobs-and-schedules.md)(能動発話パイプラインとは別)
- キャラクター定義(ベースライン等) → [../character/package-format.md](../character/package-format.md)

