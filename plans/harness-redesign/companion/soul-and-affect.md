# soul と感情(ハイブリッドモデル)

> 実現する要件: **P-301..P-308**(感情)、**P-209**(関係性の蓄積)、
> P-105 のゲート入力、P-305(感情の自己報告、[inner-channel.md](inner-channel.md) 経由)。

## 1. 設計の立場(ハイブリッド)

airi / Open-LLM-VTuber は「今この瞬間の感情ラベル」をモデルに
出させる方式(即時性が高いが、時間経過・疲れ・関係性の蓄積がない)。
現行 Ene は PAD 連続空間の内部状態(一貫性が高いが、モデルの
自己表現との接点が弱い)。

Ene は両者を2層で統合する。

| 層 | 内容 | 誰が見るか |
|---|---|---|
| **対外層(離散ボキャブラリ)** | 感情名+強度の有限集合。モデルが内面チャネルで**自己報告**し、アバターが表情として**消費**する | モデル(出力)、アバター(入力)、UI(任意表示) |
| **内部層(PAD 状態)** | 連続値の状態ベクトル。減衰・関係メトリクス・疲労を時間で動かす | システムのみ(数値はユーザーに見せない) |

接合規則:

1. モデルの自己報告(`inner/message` の `emotion` aspect)は、
   **内部状態への入力イベント**になる(§4 の分類器と合算)。
2. 内部状態は**能動発話の閾値・応答トーン・出力裁定**に影響するが、
   数値そのものをプロンプトに載せない。載るのは「感情要約」
   (System Context の `character_state` Source、粗い言葉への変換)。
3. アバターへの出力は、自己報告(即時)と内部状態(持続)の
   **裁定器**が合成する(§6)。

## 2. soul の永続構造

| フィールド | 型 | 説明 |
|---|---|---|
| `soul_id` | `SoulId` | 一意識別子 |
| `character_ref` | パッケージ参照 | 人格定義([../character/package-format.md](../character/package-format.md)) |
| `body_ref` | `BodyId?` | 結合中の body。なし=テキスト専用モード |
| `affect_baseline` | PAD+関係値 | カードのベースライン(減衰の帰着点) |
| `affect_state` | `AffectState` | 現在の内部状態(§3) |
| `memory_ref` | ボールト ID | [memory.md](memory.md) |
| `voice_ref` | 音声設定 | [../body/voice.md](../body/voice.md) |
| `skill_refs` | [skill 参照] | [../tasks/skills.md](../tasks/skills.md) |

## 3. `AffectState`(内部層の全フィールド)

| フィールド | 範囲 | 説明 |
|---|---|---|
| `valence` | -1..1 | 快-不快 |
| `arousal` | -1..1 | 覚醒度 |
| `dominance` | -1..1 | 支配性 |
| `trust` | -1..1 | ユーザーへの信頼(ゆっくり動く) |
| `affinity` | -1..1 | 親密度(ゆっくり動く) |
| `irritation` | 0..1 | 苛立ち |
| `curiosity` | 0..1 | 好奇心 |
| `fatigue` | 0..1 | 疲労(時間帯と活動量で増減) |
| `mood_label` | 離散ボキャブラリ | 支配的な感情ラベル(裁定器の出力キャッシュ) |
| `last_report_ts` | RFC3339 | 最後の自己報告/更新時刻 |

保存は soul ごとに 1 行(`<data>/companions.db` の `souls` テーブル、
[memory.md §3](memory.md) と同 DB)。更新頻度はターン境界と
定期減衰のみ(毎フレームではない)。

## 4. 更新規則

### 決定論的更新(常時)

| 事象 | 効果(既定値) |
|---|---|
| ユーザーの肯定的発話(感謝・褒め) | `valence +0.05..+0.15`、`affinity +0.002` |
| ユーザーの否定的発話(叱責・侮辱) | `valence -0.05..-0.2`、`irritation +0.1`、`trust -0.005` |
| 約束の履行/破棄 | `trust ±0.01` |
| 長時間の作業(job 連続) | `fatigue +0.05/h` |
| quiet hours 中の沈黙 | `fatigue -0.1/h`(回復) |

事象の検出は、(a) 既存の決定論ルール、(b) ターン後の LLM 分類器
(`ai.tasks.classify` で選択、既定は安価なローカル/小モデル)、
の両方。分類器の出力は**提案**であり、決定論と合算してクランプする。

### 減衰(P-302)

```text
state(t) = baseline + (state(t0) - baseline) * exp(-(t - t0) / tau)
```

- `tau` は成分ごと: `valence/arousal/dominance` は 6 時間、
  `irritation/curiosity/fatigue` は 3 時間、`trust/affinity` は
  **減衰しない**(関係性は蓄積のみ。忘れさせない)。
- 減衰は「読み出し時の現在時刻への投影」として計算する
  (保存値は最後に更新した時刻の値)。ターン境界で評価。

## 5. 離散感情ボキャブラリ(対外層)

- 有限集合(既定 24 種): `happy, joyful, excited, amused, content, calm,
  relaxed, sleepy, bored, curious, interested, surprised, confused, worried,
  anxious, sad, lonely, disappointed, embarrassed, shy, angry, annoyed,
  jealous, determined`。
- 各ラベルは PAD 空間の**参照点**(centroid)を持つ(パッケージ側で
  上書き可能、[../character/package-format.md](../character/package-format.md))。
- モデルへの提示: 内面チャネルの出力契約として
  「`emotion: <label>(intensity: 0..1)` の形式で報告できる」と教える。
  強度は省略可(既定 0.6)。
- ボキャブラリ外の報告は最近傍ラベルに正規化し、強度を 0.7 倍に
  減衰(逸脱の抑止)。

## 6. 出力裁定(P-304)

アバターへ送る感情表出キューを合成・検証する。

1. **入力**: 自己報告(即時・高優先)、内部状態の閾値越え(持続・低優先)。
2. **合成**: 自己報告は内部状態の `mood_label` を更新してから発火。
   競合時は自己報告が勝つ(モデルの意図を尊重)。
3. **検証**: ラベルが body の感情→表情写像に存在すること
   ([../body/body-and-performance.md §4](../body/body-and-performance.md))。
   存在しないラベルは最近傍の写像へフォールバック。
4. **ヒステリシス**: 同一ラベルの再発火は最小間隔 2 秒、
   ラベル変更の最小間隔 0.8 秒。強度差 0.2 未満の更新は吸収する。
5. **レート制限**: 表情変更は最大 30 回/分。超過はマージ。

## 7. 感情の可視化と編集(P-307, P-308)

- ユーザー向けの感情表示は**粗い言葉**(「ごきげん」「ちょっと疲れてる」)
  に変換したもので、数値は出さない。表示は opt-in(既定 off)。
- ベースラインの編集(P-308, stretch): 設定 UI から `affect_baseline` を
  調整。変更は監査ログに残る。

## 8. コンパニオン間の関係と家庭モデル

- soul 同士は**既定では関係を持たない**(メモリ非共有)。
- 共有スペース([memory.md §4](memory.md#4-共有スペースp-207))を
  介した情報のやり取りのみが公式な相互影響の経路。
- 「家庭」(複数 soul の緩いグループ)は**メタデータのみ**:
  名前・参加 soul の列挙・共有スペースの既定紐付け。
  関係メトリクス(trust 等)はユーザー↔soul のみで、soul 間は持たない
  (複雑化の抑止)。同室会話(P-114)は後継。

## 9. 設定キーと既定値

| キー | 既定 | 説明 |
|---|---|---|
| `mind.affect.classifier` | `auto` | 分類器プロバイダ(`auto` = ai.tasks.classify) |
| `mind.affect.decay.tau_fast_h` | `3` | 速い成分の減衰時定数 |
| `mind.affect.decay.tau_slow_h` | `6` | 遅い成分の減衰時定数 |
| `mind.affect.arbiter.min_interval_ms` | `800` | ラベル変更最小間隔 |
| `mind.affect.arbiter.max_per_minute` | `30` | 表情変更レート上限 |
| `mind.affect.visible_to_user` | `false` | 感情の粗表示(opt-in) |

## 10. 障害モード

| 障害 | 挙動 |
|---|---|
| 分類器の失敗 | 決定論のみで更新。失敗はライフサイクル警告(会話は止めない) |
| 写像にないラベル | 最近傍フォールバック+ライフサイクル警告(身体と人格の不整合の検知材料) |
| 自己報告の洪水 | レート制限で吸収。異常頻度は疲労増加の材料 |
| 減衰計算の時計巻き戻し | `last_report_ts` より前は減衰なし(前回値を維持)+警告 |

---

- 次: [inner-channel.md](inner-channel.md)

