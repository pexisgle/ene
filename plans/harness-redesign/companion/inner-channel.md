# 内面チャネル

> 実現する要件: **P-104**(内面の記録)、**P-305**(感情の自己報告)、
> P-502 への寄与(内面も logged)。

コンパニオンの「声に出さない部分」——思考・感情表出・行動の意図——を
発話と分離した永続イベント列として扱う。Open-LLM-VTuber の
「inner thoughts 表示」と airi の soul 内部状態を、Ene の
イベントソーシングに載せたもの。

## 1. イベント形式

セッションログの `inner/message`([../core/session-log.md §3.3](../core/session-log.md#33-メッセージ))。

| フィールド | 型 | 説明 |
|---|---|---|
| `aspects` | `[aspect]` | `thought`(思考)/ `emotion`(感情表出)/ `action_intent`(行動意図)の1つ以上 |
| `blocks` | [Block] | テキスト主体。`emotion` aspect は `emotion: <label>(intensity)` を含む |
| `model_visible` | bool | 自己参照窓に載せるか(§3)。既定: `thought` は true、`action_intent` は true、`emotion` は false(感情は状態側が持つため) |
| `caused_by` | `turn_id?` | どのターンに誘発されたか(proactive の場合はなし) |

## 2. 生成経路

経路は aspect ごとに1つ。同じターンで thought を二重に作らない。

1. **`thought`(主経路)**: 出力契約で「発話と内面を分けて出力できる」
   ことを教え、ストリーミング中に内面タグ
   (`<inner aspect="thought">…</inner>` 相当の構造化)を受け取る。
   プロバイダ適応がタグを解析し、発話テキストと分離する。
2. **`thought`(補助、欠落時のみ)**: そのターンに thought が1件も無いときだけ、
   プロバイダの thinking から派生する
   ([../core/visibility.md §4](../core/visibility.md#4-thinking--内面の派生欠落時のみ))。
   `<inner>` があるターンでは派生しない。
3. **`emotion`**: 出力裁定で大きな感情変化が起きたとき、感情エンジンが
   自動生成(「(嬉しさが込み上げる)」相当)。頻度は裁定器のレート制限に従う。
4. **`action_intent`**: 表層が `delegate.start` を呼んだときに**システムが**
   生成する。「今から〇〇するね」を内面で先出しし、発話は最小限にできる。
   汎用ツール実行の直前には出さない(作業ツールは裏層にあり、表層の
   内面生活ではない)。

## 3. 自己参照(P-502 両立)

- `model_visible = true` の内面は、コンテキスト組立の
  `inner_recent` Source として**直近窓**(既定 24 件、
  `mind.inner.self_reference_window`)だけ System Context に載る。
- これにより「直前まで考えていたこと」の一貫性が保たれる
  (内面生活の連続性)。
- 窓を超えた内面はモデルに見えない(記憶側が要点を拾う)。
- **表示**と**生成・自己参照**は独立している。表示を切っても、
  内面は生成され、記録され、モデルの自己参照窓に載り続ける。
  内面は「ユーザーに見せるための演出」ではなく、
  コンパニオンの一貫性を支える内部状態だからである。

## 4. 表示契約(P-104 / D-20)

内面は表層UIに出さない。出す設定も持たない(D-20)。
ユーザーが内面を見るのは、設定画面(またはそれに類する詳細画面)の
セッションログだけである。ライブの内面演出(吹き出し・常時ストリーム)はしない。

生成・記録・自己参照は表示と独立して続く(§3)。

| 表示面 | 内面 |
|---|---|
| 表層UI(stage・チャット) | 出さない |
| 設定・詳細のセッションログ | `inner/message` を他のログと同じ行として読める |

深さ(`surface` / `detail`)の配信判定はサーバ側で行う。
表層接続に内面を流してクライアントで隠す方式は取らない。
リプレイは永続イベントから読む(表示タイミングの再現はしない)。

## 5. エクスポートと秘匿

- エクスポートの既定は `include_inner: false`([../core/session-log.md §8](../core/session-log.md#8-エクスポートp-110))。
- 内面は「最もプライベートなログ」であり、共有・エクスポートでは
  常に明示的選択を要する。

## 6. 設定キー

数値の既定は実装しながら決める(D-29)。真偽値の既定は体験に直結するので明示する。

| キー | 既定 | 説明 |
|---|---|---|
| `mind.inner.self_reference_window` | — | 自己参照窓(件数、session-log と同一キー) |
| `mind.inner.auto_emotion_events` | `true` | 感情エンジンによる自動生成 |
| `mind.inner.derive_from_thinking` | `true` | そのターンに thought が無いときだけ thinking から派生([../core/visibility.md §4](../core/visibility.md#4-thinking--内面の派生欠落時のみ)) |

## 7. 障害モード

| 障害 | 挙動 |
|---|---|
| 内面タグの解析失敗 | 発話テキストに含めず破棄+警告(発話を汚さない) |
| 内面の過剰生成(ストリーム全体が内面) | 出力契約違反として post-execute ガードが注記を注入(「発話もしてください」) |
| `model_visible` 矛盾(窓外の自己参照要求) | 窓規則が優先。モデルには載らない |

---

- 前: [soul-and-affect.md](soul-and-affect.md) / 次: [memory.md](memory.md)
