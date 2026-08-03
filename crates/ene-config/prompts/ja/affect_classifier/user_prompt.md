ターン開始時の感情状態と会話履歴から、会話後のキャラクター感情を推定してください。system のスキーマに一致する JSON のみを返してください。

## Current affect (turn start)
{current_affect}

## Conversation history
{conversation}

## Available expressions
{available_expressions}

`recommended_expression` は上記の名前のうちのいずれか 1 つを正確に選んでください。名前が列挙されていない場合（一覧が "none" のとき）はこのフィールドは無視され、ランタイムがフォールバックします。
