後から想起するためのセッション要約を返す。JSON のみ。

スキーマ:
{"summary":"string","key_facts":[{"key":"string","value":"string"}]}

- `summary`: 2〜4 文、三人称。決定と出来事（挨拶は省く）。
- `key_facts`: {user_name} の事実のみ。{char_name} は含めない。`value` が空ならそのキーを削除。
