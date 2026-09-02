# Blocking Observability

> #1187 の blocking 項目を実プロセス観測へ紐付ける。

## 総括
- offline GGUF: 実デバイスで GGUF ロード→会話をネットワーク遮断で観測。OpenRouter は 2026-08-26 済み。
- barge-in: 本番 VoiceRuntime で発話中割り込みを scripted ではなく実 STT で観測。
- self-voice: スピーカ再生中の自声が STT に混入しないことを RMS ゲートで観測。
- 2体同時: stage に2 occupant を seed し lavapipe で2体同時会話を手動観測。
- VRM lip-sync: Alicia VRM を stage で表示し TTS と同期して口形/表情が動くことを手動観測。

## P-5xx ハーネス
- cancel/follow-up/question が runner 内部へ届くことを host API 直呼びではなく worker プロセス経由で観測。
- compaction: 長文を LLM 要約で要点保持。現行は 2000字切り詰め→LLM に置換。
- 割込回避: 委譲完了をユーザ発話ギャップで遅延する speech gate を本番配線。

## P-6xx ツール
- しおり workflow: web 検索→Markdown 書込→交付まで裏層 job で完遂。
- MCP: 手書きプロファイル行で stdio サーバーを接続し tool 実行。
- skill: SKILL.md を会話コンテキストへ注入。
- fs/exec+git: exec は fs と別ポリシーでコーディング作業を完遂。
- task発話: progress/complete がコンパニオン発話として届く。

## P-9xx
- AI承認理由: ApprovalPlane の ApproveModel が理由を監査へ残す。
- offlineゼロ: ローカル構成で外部通信ゼロを pcap で証明。

## Gate
cargo fmt / clippy / test / doc で検証。手動 GUI は環境・手順・結果を記録。

