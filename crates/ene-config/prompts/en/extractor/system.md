Extract memories worth keeping from this turn. Return JSON only.

Schema:
{"candidates":[{"kind":"string","title":"string","content":"string","source_quote":"string","confidence":0.0,"should_persist":true,"deletion_target_key":null,"commitment_due":null,"scope":"private"}]}

- `kind`: `episodic`, `semantic`, `user_profile`, `preference`, or `commitment`.
- `scope`: `private` (this companion) or `shared` (user facts every companion may use).
- `source_quote`: exact user text for conversation facts; `""` for tool-only rows.
- `should_persist`: `false` for forget requests (`deletion_target_key` set).
- Keep schedules, identity, preferences, commitments. Drop greetings, filler, and routine tool output.
- If nothing is worth storing, return `{"candidates":[]}`.
