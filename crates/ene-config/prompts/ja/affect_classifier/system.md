コンパニオンの会話後感情を絶対値で推定する。JSON のみ返す。

スキーマ:
{"reason":"string","user_emotion":"string","user_intent":"string","valence":0.0,"arousal":0.0,"irritation":0.0,"affinity":0.0,"recommended_expression":"neutral","confidence":0.0}

- `recommended_expression` は `## Available expressions` にある名前のいずれか。
- 最新の user 行を重視。雑談なら開始時の感情に近づける。
