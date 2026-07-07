# アーキテクチャ概要

ene は `ene-core` の実行オーケストレーションと `ene-cognition` の認知ランタイムを中心に構成されたモジュラー Rust ワークスペースです。

## ランタイム構成

実行シェルは引き続きアクターモデル（`EneHandle` / `EneActor`）ですが、ターンごとの知的処理は cognition コンポーネントが担当します。

### ターン処理フロー

```text
ユーザー入力
  -> before_turn（recall 計画 + affect 更新）
  -> compose_prompt_packet（セクション化文脈 + 予算管理）
  -> LLM ストリーミング
  -> output arbitration（エンジン管理表情）
  -> after_turn（memory 書き込み + 忘却 + affect 永続化）
```

`ene-core` はこのフローを streaming lifecycle に統合し、desktop/CLI へイベントを配信します。

## 主要クレート

- `ene-core`: アクターランタイム、ストリーミング、イベント、ツール実行。
- `ene-cognition`: recall planner、prompt packet、emotion engine、output arbiter、memory writer、context compression。
- `ene-memory`: typed memory 永続化、hybrid search、commitment/affect ストア。
- `ene-session`: 会話セッション状態と互換 split/compression フック。
- `ene-provider`: LLM/埋め込みプロバイダ抽象。
- `ene-tool-*`: サンドボックス化されたツール実行と IPC プロトコル。

## メモリモデル

typed memory（`episodic`、`semantic`、`preference`、`commitment` など）を使用し、状態は `active` / `faded` / `archived` / `disputed` / `superseded` / `user_deleted` で管理されます。

hybrid recall は以下を合成します。

- ベクトル類似度
- 語彙一致
- 時間減衰
- salience / confidence
- affect / commitment シグナル

## プロンプトモデル

プロンプトは `PromptPacket` としてセクション化され、明示的なトークン予算で管理されます。Identity と output contract は予算超過時にも保持されます。

## 感情・表情モデル

- Affect state はエンジン側で永続化
- LLM 分類器は任意かつ advisory
- 最終表情は Output Arbiter がヒステリシス付きで決定
- UI 側は `EneEvent::Expression` を受信して反映

## アプリケーション

- `ene-cli`: メモリ/感情/コミットメントのデバッグコマンドを含む REPL。
- `ene-desktop`: `winit` + `wgpu` + `egui` + VRM 表示、認知デバッグ UI を提供。

## 参照

設計詳細は `docs/ja/architecture/cognitive-runtime.md` を参照してください。
