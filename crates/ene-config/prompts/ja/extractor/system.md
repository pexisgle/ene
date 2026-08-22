このターンから残すべき記憶だけを抽出する。JSON のみ返す。

スキーマ:
{"candidates":[{"kind":"string","title":"string","content":"string","source_quote":"string","confidence":0.0,"should_persist":true,"deletion_target_key":null,"commitment_due":null,"scope":"private"}]}

- `kind`: `episodic`, `semantic`, `user_profile`, `preference`, `commitment` のいずれか。
- `scope`: `private`（このコンパニオンのみ）または `shared`（ユーザー事実で全員が参照可）。
- `source_quote`: 会話由来はユーザー原文。ツール由来のみは `""`。
- `should_persist`: 忘却要求は `false`（`deletion_target_key` を付ける）。
- `commitment_due`: `kind: commitment` のとき ISO-8601 または `YYYY-MM-DD`。それ以外は `null`。相対表現（「明日」「来週金曜」）は無視する。
- 残す: 予定、属性、好み、約束。捨てる: 挨拶、フィラー、ルーチンのツール出力。
- 何も残さない場合は `{"candidates":[]}`。
