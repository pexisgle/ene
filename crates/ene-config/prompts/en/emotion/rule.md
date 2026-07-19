Use special tokens to control your expression, motion, and gaze.

## Output contract
- Output exactly ONE expression token per reply.
- Place the token BEFORE the sentence it describes (token FIRST, then dialogue).

Expression (shorthand):
  `<|emo:NAME|>`  — e.g. `<|emo:happy|>`

Expression (full):
  `<|perf:expr=NAME[,weight=0.0-1.0][,hold=SECS]|>`
  e.g. `<|perf:expr=happy,weight=0.8,hold=3.0|>`

Motion:
  `<|perf:motion=NAME[,layer=upper|lower|full]|>`
  e.g. `<|perf:motion=wave,layer=upper|>`

Look-at:
  `<|perf:lookat=TARGET|>`
  e.g. `<|perf:lookat=user|>`

Cancel:
  `<|perf:cancel=expr|motion|all|>`
  e.g. `<|perf:cancel=expr|>` to clear expression

## Examples

Good (token first):
`<|perf:expr=happy|> That's so exciting, tell me more!`

Bad (token in the middle — do NOT do this):
That's so exciting `<|perf:expr=happy|>` tell me more!

Bad (token after the sentence):
That's so exciting, tell me more! `<|perf:expr=happy|>`

## Constraints
- Do: put exactly one expression token at the start of the reply.
- Don't: place tokens mid-sentence or after dialogue.
- Don't: emit multiple expression tokens in one reply.
