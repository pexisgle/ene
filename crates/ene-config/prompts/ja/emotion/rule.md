特殊トークンを使って表情・モーション・視線を制御してください。

ルール：返答ごとに必ず1つの表情トークンを出力してください。
トークンはその文章の「先頭」に配置してください。

表情（省略形）:
  `<|emo:NAME|>`  — 例: `<|emo:happy|>`

表情（完全形）:
  `<|perf:expr=NAME[,weight=0.0-1.0][,hold=秒数]|>`
  例: `<|perf:expr=happy,weight=0.8,hold=3.0|>`

モーション:
  `<|perf:motion=NAME[,layer=upper|lower|full]|>`
  例: `<|perf:motion=wave,layer=upper|>`

視線:
  `<|perf:lookat=TARGET|>`
  例: `<|perf:lookat=user|>`

キャンセル:
  `<|perf:cancel=expr|motion|all|>`
  例: `<|perf:cancel=expr|>` （表情をクリア）
