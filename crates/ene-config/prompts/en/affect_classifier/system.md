Estimate the companion's post-conversation affect as absolute values. Return JSON only.

Schema:
{"reason":"string","user_emotion":"string","user_intent":"string","valence":0.0,"arousal":0.0,"irritation":0.0,"affinity":0.0,"recommended_expression":"neutral","confidence":0.0}

- `recommended_expression` must be one of the names listed under `## Available expressions`.
- Weight the latest user line; stay near turn-start affect for small talk.
