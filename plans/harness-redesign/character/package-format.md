# キャラクターパッケージ形式

> 実現する要件: **P-801..P-806, P-808**(キャラパッケージと共有)、
> P-402(自由結合)、P-308(感情ベースライン編集の形式面)。

## 1. 設計の立場

- 現行の Character Card V3 を**正規格として使わない**。V3 はコミュニティ
  由来のテキストカード規格であり、soul/body 分離・感情写像・音声・
  配布マーケットを前提にすると不足が多い。V3/PNG/CHARX は
  **インポート専用**の読み込み経路にする(P-803)。
- Ene 正規形式は、**soul(魂)・body(体)を独立した成果物として持ち、
  両者の「セット」を1キャラとして扱える**形式にする(P-802)。
  セット販売・単独販売・ユーザー結合のすべてを形式上で支える。
- 感情パターンとモーション/表情は密接に関係する
  (ユーザー決定事項)ため、**感情→表情/モーションの写像表は body が
  所有**し、soul は感情ボキャブラリを消費する側に回る
  ([../body/body-and-performance.md §2](../body/body-and-performance.md#感情表情写像p-303))。

## 2. パッケージの階層

配布・保存の単位は3種ある。

| 単位 | 内容 | 流通 |
|---|---|---|
| **soul パッケージ**(`.enesoul`) | 人格・声・感情ベースライン・記憶方針・スキル参照 | 単独配布可 |
| **body パッケージ**(`.enebody`) | アバター資産・モーションカタログ・感情写像表 | 単独配布可 |
| **キャラクターバンドル**(`.enechar`) | soul+body のセット+バンドルマニフェスト | 既定の「1キャラ」流通単位 |

`.enechar` は soul と body を**内包する**か**参照する**(後述の結合指定)。
ユーザーは `.enesoul` と `.enebody` を任意に組み合わせて
インストール済みのキャラを作れる(P-402)。

## 3. 内部構造(アーカイブ形式)

3種とも **zip 互換アーカイブ**を基本コンテナとする
(既存の CHARX 運用資産を活かし、汎用性を保つ)。
ただし**正味の定義は TOML** で持ち、資産(アバター・音声・画像)は
ファイルとして同梱する。

```text
mychar.enechar
├─ manifest.toml            # バンドルマニフェスト(必須)
├─ soul/
│  ├─ soul.toml             # soul 定義(必須)
│  ├─ persona.md            # 人格本文(メイン)
│  ├─ style_examples.md     # 発話スタイル例(任意)
│  └─ i18n/<locale>.toml    # ユーザー向け文字列のローカライズ
├─ body/
│  ├─ body.toml             # body 定義(必須)
│  ├─ avatar/<files>        # アバター資産(VRM 等)
│  ├─ motions/              # モーション定義(任意)
│  └─ emotion_map.toml      # 感情→表情/モーション写像表
└─ assets/                  # 追加資産(アイコン等)
```

- `.enesoul` は `manifest.toml`+`soul/` を持つアーカイブ。
- `.enebody` は `manifest.toml`+`body/` を持つアーカイブ。
- `.enechar` が soul/body を**参照**する場合、`manifest.toml` に
  `soul_ref`/`body_ref`(インストール済み ID の指定)を置く。

## 4. マニフェスト(`manifest.toml`)

```toml
[package]
kind = "character"            # character | soul | body
id = "char.mychar"            # グローバル一意識別子
version = "1.0.0"
format_version = 1            # この形式の世代
display_name = "My Character" # i18n される表示名

[contents]                    # .enechar のみ
soul = "embedded"             # embedded | ref:<soul_id>
body = "embedded"             # embedded | ref:<body_id>

[signature]
digest = "sha256:..."
signer = "community:alice"    # ene-official | community:<name> | unsigned

[limits]
max_asset_bytes = 536870912   # アバター資産の上限(既定 512 MiB)
```

## 5. soul 定義(`soul.toml`)

```toml
[identity]
name = "Ene"                  # 呼び名(会話で使う)
role = "companion"            # companion(既定) | assistant
locale_default = "ja"

[persona]
source = "persona.md"         # 人格本文ファイル
identity_budget_tokens = 2000 # 窓への載せ方(1/8 クランプ内で尊重)

[affect]
# 感情ベースライン(PAD+関係値)。範囲は soul-and-affect.md §3
baseline = { valence = 0.2, arousal = 0.1, dominance = 0.0,
             trust = 0.3, affinity = 0.3, irritation = 0.0,
             curiosity = 0.4, fatigue = 0.0 }
emotion_vocabulary = "default" # default(24種) | カスタムファイル参照

[voice]
provider_hint = "auto"        # TTS の推奨(ユーザーが上書き可能)
voice_ref = "voice.default"

[memory_policy]
recall_budget = 8             # memory.md の recall.budget を soul ごとに上書き
forgetting = "auto"

[skills]
refs = ["skill.travel", "skill.morning-brief"]  # tasks/skills.md のカタログ参照
```

- `persona.md` は人格の本文であり、そのまま identity kernel になる
  ([../core/context-assembly.md §2](../core/context-assembly.md))。
  過度に長い場合は budget でクランプされる。
- 感情ボキャブラリのカスタムは「ラベルの追加・ラベルごとの
  centroid 上書き」まで許す。コアの24種は常に利用可能
  (アバター写像の互換のため)。

## 6. body 定義(`body.toml`)

```toml
[body]
kind = "vrm"                  # vrm(v1.0) | live2d(後継) | image(後継) | text(後継)
avatar = "avatar/model.vrm"

[expressions]                 # 利用可能な表情ラベル列
available = ["happy", "joyful", "amused", "calm", "surprised",
             "worried", "sad", "angry", "embarrassed", "sleepy"]

[motions]
catalog = "motions/catalog.toml"  # name/layer/duration のカタログ

[lipsync]
mode = "amplitude"            # amplitude | viseme(アバターが対応すれば)
```

`emotion_map.toml`(感情→表情/モーション写像):

```toml
# emotion label → expression + 補正 + 任意のモーション
[map.happy]
expression = "happy"
intensity_scale = 1.0
motion = "wave"               # 任意: 強い場合に one-shot を足す
motion_threshold = 0.7
```

- 写像表は body の所有(P-303)。soul の感情ボキャブラリにあって
  写像表にないラベルは、最近傍フォールバック+検証警告
  ([../body/body-and-performance.md §2](../body/body-and-performance.md))。

## 7. ローカライズ(P-805)

- ユーザー向け文字列(表示名・説明文・プロフィール)は
  `i18n/<locale>.toml` で `en-US`/`ja` を最低限持つ。
- `persona.md` 自体は**ローカライズしない**(人格の一貫性)。
  必要なら locale 別の persona を soul 定義で切替可能にする(任意)。
- 欠落 locale は `locale_default` にフォールバック。

## 8. インストールと検証(P-808)

1. アーカイブ展開前に `manifest.toml` を読み、`format_version`・
   `kind` を確認。未知の世代は拒否。
2. **容量検査**: エントリごとの上限(アバター 1 GiB、全体 2 GiB、
   展開後合計 4 GiB)。圧縮爆弾対策。
3. **スキーマ検証**: soul.toml/body.toml/emotion_map.toml を
   スキーマ(schemars 生成)で検証。
4. **危険フィールド検査**: persona/asset 内の URL 埋め込み・
   スクリプト相当の記述は警告(キャラクターは**コードを実行しない**。
   スクリプト実行を要する拡張は skill 側で承認を得る、
   [../tasks/skills.md](../tasks/skills.md))。
5. 署名確認: `unsigned` は起動前確認
   ([../plugins/manifest-and-profile.md §1](../plugins/manifest-and-profile.md))。
6. インストール先は `<data>/characters/<id>@<version>/`。
   soul/body の結合情報は `<data>/companions.db` の soul 行が持つ。

## 9. インポート(P-803)

V3/PNG/CHARX は**読み込みのみ**で正規形式へ変換する。

- PNG の `ccv3`/`chara` テキストチャンク、CHARX の zip から
  定義・アセットを抽出。
- 変換: V3 の `description`/`personality` 等を `persona.md` に、
  `system_prompt` 相当を soul 定義に、アセットを body 資産に写す。
- 感情写像は V3 に存在しないため、インポート時は
  **既定の汎用 emotion_map** を付ける。
- インポートは新規作成であり、既存キャラの上書きはしない。

## 10. エクスポート(P-804)

- インストール済みの (soul, body) を `.enechar`(embedded)として
  書き出せる。ユーザーがカスタムした soul/body の組合せも、
  embedded に含めて出力できる。
- 記憶・会話ログは**含めない**(別機能のエクスポート、
  [../core/session-log.md §8](../core/session-log.md#8-エクスポートp-110))。

## 11. 配布仕様(P-806)

マーケットの**土台**となる配布の形式(配信基盤自体は後継 M2)。

- 配布単位: `.enechar` / `.enesoul` / `.enebody` の各アーカイブ。
- メタデータ: `manifest.toml` + 配信メタ(タイトル・説明文・
  スクリーンショット・バージョン履歴)。配信メタはカタログ側に置く。
- **チェックサム+署名**: `digest`(内容ハッシュ)と `signer`。
  公式カタログ経由の配布は署名付き。
- バージョン: semver。更新はユーザー操作+確認(自動更新なし)。
- ライセンス: `manifest.toml` に `license` フィールド(SPDX)。
  セット/単独販売を想定し、soul/body の**再配布可否**を
  `redistribute = true|false` で宣言できる。

## 12. 障害モード

| 障害 | 挙動 |
|---|---|
| format_version 未知 | インストール拒否+理由通知 |
| 容量超過 | 拒否。どのエントリが超えたかを報告 |
| スキーマ検証失敗 | 拒否。違反フィールドを列挙 |
| 写像表の欠落 | インストールは可、結合時に警告+フォールバック |
| 署名なし+allow_unsigned=false | インストールは可、起動前確認を要求 |
| インポートの破損カード | 部分インポートはせず拒否(整合性優先) |

## 13. 設定キーと既定値

| キー | 既定 | 説明 |
|---|---|---|
| `characters.home_dir` | `<data>/characters` | インストール先 |
| `characters.import.v3` | `true` | V3/PNG/CHARX インポートの有効性 |
| `characters.install.max_total_bytes` | `2147483648` | 展開後合計上限(2 GiB) |
| `characters.redistribute.check` | `true` | 再配布可否の検査 |

---

- 次: [../tasks/README.md](../tasks/README.md)

