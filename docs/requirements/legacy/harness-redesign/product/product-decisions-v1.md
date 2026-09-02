# Product Decisions v1

> PC-D1..D6 を D番号付きで正式化し v1 境界を 10 体験で言語化。

## PC-D1..D6 (仮D番号割当)
- D-40 初期OS: Linux + native Windows を v1 とする。mac は対象外。
- D-41 推奨モデル: OpenRouter llama-3.3-70b を推奨、ローカル GGUF を offline 代替。
- D-42 v1音声: 全二重の barge-in / self-voice 回避を v1 に含める。
- D-43 複数 companion: 2体同時会話を v1、M1 で同室会話へ。
- D-44 Web: 同一 LAN からの対話は v1、設定変更は desktop のみ。
- D-45 完成形: 10体験で説明 -> 会話/割込/2体/記憶/感情/VRM/job並行/取消/しおり/detail/安全。

## 同期
- vision.md / features.md / done.md / decisions.md が同じ v1 を指すことをこの文書で担保。
- 次の実装単位は #1201 垂直 slice から選ぶ。

