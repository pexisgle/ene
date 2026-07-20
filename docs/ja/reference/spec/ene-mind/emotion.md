# `EmotionEngine` / PADモデル感情状態仕様

本ドキュメントでは、Ene の感情状態を管理する PAD (Pleasure-Arousal-Dominance) モデルおよび、確定条件（Appraisal）と LLM 分類器を組み合わせた感情状態更新プロセスの仕様を定義します。

---

## 1. 感情空間モデル (PAD Space)

Ene の感情状態は、以下の連続値（P-A-D + 追加指標）で構成され、データベースの `affect_states` テーブルに保存されます。

*   **Valence (Pleasure) [-1.0 ..= 1.0]**: 愉悦（ポジティブ）〜不快（ネガティブ）。
*   **Arousal [-1.0 ..= 1.0]**: 覚醒（興奮）〜睡眠（沈静）。
*   **Irritation [0.0 ..= 1.0]**: イライラ度・ストレス蓄積。
*   **Affinity [-1.0 ..= 1.0]**: ユーザーに対する好意・親密度。
*   **Fatigue [0.0 ..= 1.0]**: 疲労度。

---

## 2. 感情更新ライフサイクル (`update_turn`)

毎ターン、`before_turn` 内で `EmotionEngine::update_turn` が呼び出され、以下の順序で状態を更新します。

```rust
pub fn update_turn(
    &self,
    config: &EmotionConfig,
    input: &mut TurnAffectInput<'_>,
) -> AffectUpdateResult
```

### 1. 時間経過による減衰 (`apply_decay`)
*   **物理モデル**: 感情は時間経過に伴い、徐々に基準値（ニュートラル = 0.0）へ収束します。
*   **減衰式**:
    $$V_{\text{new}} = V_{\text{old}} \times e^{-\lambda t}$$
    ここで $\lambda = \frac{\ln(2)}{\text{decay\_half\_life\_minutes}}$ です。経過時間 `elapsed_since_update` に基づき、各パラメーターが減衰します。

### 2. 決定論的査定 (`apply_appraisal`)
入力テキストの特徴から、簡易的な感情変動（アプレイザル）を即座に適用します。
*   **覚醒度の上昇**: 「！」の連続、感嘆符、大文字英語の多用により Arousal が上昇。
*   **イライラの上昇**: ユーザー入力の文字数が極めて長い場合、または特定拒絶語によって少量の Irritation が蓄積。
*   **親密度の変動**: 感謝語「ありがとう」や肯定的なフレーズにより Affinity が上昇。

### 3. LLM分類器のブレンド (`merge_classifier_proposal`)
*   **線形補間ブレンド**:
    前ターン完了後にバックグラウンドでLLMが推論した `AffectProposal`（提案感情値）をブレンドします。
    確信度 `confidence`（0.0 〜 1.0）をブレンドの重み $w$ とし、以下の加重平均式で適用します。
    $$X_{\text{new}} = (X_{\text{proposal}} - X_{\text{current}}) \times w + X_{\text{current}}$$
    これにより、LLMの自信度（確信度）が高い推論ほど、現在の感情値に強く反映されます。

### 4. ムードラベルの計算 (`compute_mood_label`)
更新後の P-A-D-Affinity の各次元の極性としきい値から、マスコットの現在の気分（ムード）を決定論的にラベリングします。
*   `Joyful` (Valence > 0.3, Arousal > 0.0)
*   `Relaxed` (Valence > 0.2, Arousal <= 0.0)
*   `Anxious` (Valence < -0.2, Arousal > 0.2)
*   `Depressed` (Valence < -0.2, Arousal <= 0.2)
*   `Hostile` (Irritation > 0.5)
*   `Neutral` (それ以外)

---

## 3. ポストターン感情分類器 (`classifier.rs`)

感情変化のより高度で文脈的な判断（嫌味の検知、会話の雰囲気の解釈など）は、LLM を用いてターン終了後にバックグラウンドタスクとして行われます。

*   **入力コンテキスト (`ClassifierContext`)**:
    -   前ターンの開始時の感情状態。
    -   今回のユーザー入力とアシスタント（マスコット）の返答。
*   **出力フォーマット**:
    LLMに対して以下の項目を持つ JSON 構造体の返却を強制します（`EmotionClassifier` 内部のスキーマ定義）。
    ```json
    {
      "user_emotion": "user's estimated emotion",
      "user_intent": "user's interaction intent",
      "valence": 0.5,
      "arousal": -0.2,
      "irritation": 0.0,
      "affinity": 0.3,
      "recommended_expression": "joy",
      "confidence": 0.85,
      "reason": "The user thanked the mascot warmly."
    }
    ```
*   **反映メカニズム**:
    生成された JSON 構造体は `PendingAffectProposal` テーブルに一時保存され、**次ターンの `before_turn` 開始時にロードされて感情値にブレンドされます**。これにより、LLMによる重たい感情査定処理が会話の応答（ストリーミング）を遅延させない仕組みを実現しています。
