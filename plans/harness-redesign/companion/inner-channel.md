# 内面チャネル

> 実現する要件: **P-104**(内面の表示)、**P-305**(感情の自己報告)、
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

1. **モデルによる生成(主経路)**: 出力契約で「発話と内面を分けて
   出力できる」ことを教え、ストリーミング中に内面タグ
   (`<inner aspect="thought">…</inner>` 相当の構造化)を受け取る。
   プロバイダ適応がタグを解析し、発話テキストと分離する。
2. **感情エンジンによる生成**: 出力裁定で大きな感情変化が
   起きたとき、`emotion` aspect の内面イベントを自動生成
   (「(嬉しさが込み上げる)」相当)。頻度は裁定器のレート制限に従う。
3. **行動意図の生成**: ツール実行前(job を受けた、調べ物を始めた等)に
   `action_intent` を出す。「今から〇〇するね」を内面で先出しし、
   発話は最小限にできる。
4. **thinking からの派生**(P-520、[../core/visibility.md §4](../core/visibility.md#4-thinking--内面の派生任意)):
   プロバイダの thinking ブロックは記録されるがユーザーには見せず、
   ターン終了後に感情エンジンが思考の要点を `thought` aspect で
   再生成する(原文コピーではなくキャラの語り直し)。

## 3. 自己参照(P-502 両立)

- `model_visible = true` の内面は、コンテキスト組立の
  `inner_recent` Source として**直近窓**(既定 24 件、
  `mind.inner.self_reference_window`)だけ System Context に載る。
- これにより「直前まで考えていたこと」の一貫性が保たれる
  (内面生活の連続性)。
- 窓を超えた内面はモデルに見えない(記憶側が要点を拾う)。
- **ユーザーに見せる表示**と自己参照は独立設定。表示を off にしても
  記録と自己参照は続く。

## 4. UI 表示契約(P-104)

- クライアントは内面を「発話とは別の視覚様式」で表示する
  (吹き出しの外・薄い色・斜体等はクライアントの自由)。
- 表示の既定は **on**(コンパニオンらしさの中核)。ユーザーが
  セッション単位・グローバル単位で off にできる。
- ライブストリームではチャンク単位で流し、確定時に永続イベントと
  対応付ける([../core/session-log.md §6](../core/session-log.md#6-ライブストリームとの分担))。
- リプレイは永続イベントから再生(表示タイミングの再現はしない)。

## 5. エクスポートと秘匿

- エクスポートの既定は `include_inner: false`([../core/session-log.md §8](../core/session-log.md#8-エクスポートp-110))。
- 内面は「最もプライベートなログ」であり、共有・エクスポートでは
  常に明示的選択を要する。

## 6. 設定キーと既定値

| キー | 既定 | 説明 |
|---|---|---|
| `mind.inner.enabled` | `true` | 内面チャネル全体の有効性(off にすると生成もしない) |
| `mind.inner.self_reference_window` | `24` | 自己参照窓(件数、session-log と同一キー) |
| `mind.inner.display_default` | `true` | クライアント表示の既定 |
| `mind.inner.auto_emotion_events` | `true` | 感情エンジンによる自動生成 |

## 7. 障害モード

| 障害 | 挙動 |
|---|---|
| 内面タグの解析失敗 | 発話テキストに含めず破棄+警告(発話を汚さない) |
| 内面の過剰生成(ストリーム全体が内面) | 出力契約違反として post-execute ガードが注記を注入(「発話もしてください」) |
| `model_visible` 矛盾(窓外の自己参照要求) | 窓規則が優先。モデルには載らない |

---

- 前: [soul-and-affect.md](soul-and-affect.md) / 次: [memory.md](memory.md)
