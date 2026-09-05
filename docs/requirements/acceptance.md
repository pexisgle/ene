# 現在の受け入れ条件

状態: **最初のmilestone**
最終確認: 2026-09-05

本書は、[製品要件](requirements.md)のうち最初に実装して検証する範囲を定める。記載のない要件を否定せず、製品全体に新しい必須挙動を追加しない。Milestone完了後は、実測結果と次のRelease計画に合わせて更新する。

## Support Matrix

| 項目 | Gate |
|---|---|
| Windows | Windows 11、x86-64 |
| Linux | NixOS 26.11.20260903.0968519、x86-64、KDE Wayland |
| UI言語 | 日本語、英語 |
| Main LLM | OpenAI Responses API |
| 同梱Character | `ene` |

この表は最初のmilestoneの検証環境であり、恒久的なProvider既定、正式な配布範囲、最低動作環境を意味しない。

## Milestone範囲

このmilestoneに含める:

- 最小Setupから最初のテキスト会話まで
- 基本的なDesktop Body
- 一対一のテキストtimeline
- Companion scopeのExperience Summary、Memory形成、由来表示、想起、継続的な更新と会話による訂正
- 一つのWorkspace folderを対象とするfile作業Task
- Companionを通じたTaskの進捗確認、steering、Cancel、結果報告
- Client不在時のTask継続と、Host再起動後の明示再開
- OpenAI利用量と費用の確認、およびCredential漏えい防止
- 主要障害時のdata保全とText管理面
- 両OSでの最低性能Gateと性能baselineの確認

このmilestoneでは後続とする:

- Hostの自動起動とbackupからの復元
- Cursor、window、画面端、複数monitorへ反応するBody
- Voice、Observation、グループ会話、自発的なCompanion間交流
- Global scopeのMemory
- ExperienceからのSkill生成とSkill改善
- 長期的なRelationship発達の評価
- Schedule、Remote Client、Character編集とPackage配布
- OpenAI以外のProviderとfallback

後続項目も[製品要件](requirements.md)の一部であり、未決定を意味しない。

## 代表的なend-to-endシナリオ

### Setupと最初の会話

1. 新規環境でEneを起動する。
2. 日本語または英語、同梱Character `ene`、Cloudへ送信されるdataと費用説明、OpenAI Credential、利用modelを選ぶ。
3. Credentialを登録しただけでは送信が始まらず、Main LLMへの割当を明示してSetupを完了する。
4. `ene` から作られたCompanionへテキストを送り、同じtimelineで応答を受け取る。
5. UI言語を切り替えても、Companionと保存済み会話が維持されることを確認する。

### 基本Body

1. 対応OSで透明overlayとしてBodyを表示する。
2. Bodyを移動、resize、hideし、通常のdesktop操作を継続できることを確認する。
3. 少なくともidleと応答中の状態を区別できることを確認する。
4. Bodyを停止または故障させても、テキスト会話と管理面を利用できることを確認する。

### 会話からのMemory

1. 一続きのテキスト会話で、複数messageにまたがる後に役立つOwnerとの出来事または継続的な好みを伝える。
2. Messageを一件ずつMemoryへ複製せず、その出来事を意味的なまとまりとして圧縮したExperience Summaryを根拠にCompanion scopeのMemoryが形成されることを確認する。
3. 管理面から、Memoryの内容、scope、時間的な意味、重要度、根拠となるExperience Summary、形成または更新時点を確認できることを確認する。
4. ClientとHostを正常終了して再起動し、関連する会話でそのMemoryを自然に想起できることを確認する。
5. Ownerが会話で内容を訂正した場合は誤っていた認識として更新でき、以前は正しかった状況が後に変化した場合は過去の有効性を失わず新しい現在状態へ更新できることを確認する。
6. 更新後のMemoryから、現在の内容だけでなく、根拠となったExperience Summaryと過去revisionを確認できることを確認する。
7. 「覚えておいて」という明示的な依頼は強い形成signalとして扱われる一方、Password、token、秘密鍵等はその依頼があってもMemoryへ保存されないことを確認する。
8. 一時的な気分、Task内だけの状態、一般世界知識、Raw Tool payload、明白に偶発的な外部情報を、長期Memoryとして無条件に形成しないことをfixtureで確認する。
9. 同じ情報を繰り返し伝えた場合は無条件に重複Memoryを増やさず、既存Memoryの補強、精密化、統合等として扱えることを確認する。

### Workspaceでのfile Task

1. Ownerが既存fileを含む一つのfolderをTaskのWorkspaceとして指定し、Companionへ既存fileを読み、新しいMarkdown成果物を同じfolderへ作るよう依頼する。
2. TaskのWorkspaceがそのTaskに従属し、Taskより上位の独立した作業containerの作成を要求しないことを確認する。
3. Task Agentには、そのfolder内の一覧、読取、新規作成、編集だけを許可する。
4. Task中に担当Companionから進捗を確認し、成果物への追加指示を送り、反映を確認する。
5. Taskを別の実行でCancelし、best-effortの結果と残った変更を確認する。
6. 正常終了時に、変更したfile、保存場所、未完了事項を担当Companionが報告することを確認する。
7. Delete、shell、Network、MCP、Workspace外のpath、path traversalが拒否され、別経路で迂回されないことを確認する。
8. Task側のWorkspace関連付けが失われても、Workspace folder内の既存fileと成果物が黙って削除されないことを確認する。

### ClientとHostのlifecycle

1. 長く実行されるfile作業Taskを開始し、Clientを閉じる。
2. Host上ではTaskが継続し、Client再接続後に進捗または結果を確認できることを確認する。
3. 別の進行中TaskでHostを終了し、再起動する。
4. Taskが自動再開せず、保存済み進捗と既知の外部作用を示したうえでOwnerの明示再開を待つことを確認する。

### 障害と安全境界

次の各条件を独立して発生させ、保存済みのConversation History、Memory、Task記録、Workspace関連付けが破損せず、Text管理面から状態と安全な次の操作を確認できることを検証する。

- 不正なCredential
- Provider停止またはNetwork切断
- Prompt cache miss
- Body rendererの停止
- Task Agentの途中失敗
- TaskのCancel
- Workspace外を指すpath traversal
- File保存途中のprocess停止

外部作用の成功が不明な失敗では自動再実行されず、Ownerへ重複riskを示すことも確認する。

### 利用量と秘密情報

1. OpenAIを利用する会話とTaskを実行する。
2. Providerが報告するinput token、output token、cached tokenと、それぞれに対応する費用内訳を管理面で確認する。
3. Provider報告値とEneの推定値または不明な値が混同されないことを確認する。
4. 登録したCredentialがLLMへ送信されるmodel contextやTool argument、Conversation History、Memory、Task結果、通常log、Error表示に現れないことを自動検査と目視の両方で確認する。

## 性能Gateとbaseline

WindowsとLinuxで同じ操作手順と観測区間を用い、少なくとも次を記録する。

- Cold startとwarm startの起動時間
- Idle時のCPU、GPU、RAM使用量
- Body表示時のCPU、GPU、frame安定性
- テキスト会話の送信から最初のtoken表示までの時間
- 同一fixtureを用いたfile作業Taskの所要時間
- 会話、Memory、Taskを反復したときの保存容量増加

最初のmilestoneでは、modelや外部Provider processを除くEne自身について次を最低Gateとする。

- Setup完了後かつ推論を行っていないidle状態を5分観測し、Ene process群の平均CPU使用率がsystem全体の10%を超えない。
- 同じidle状態で、Hostとactive Clientを合わせた常用Memoryが2 GiBを超えない。
- 基本Bodyを通常表示している間、対応環境で平均30 FPS以上を維持し、通常のUI操作を1秒を超えて継続的にblockしない。
- Stop、Cancel、Mute、Permission拒否等のlocal操作は、入力から1秒以内にUI上で受付状態または状態変化を示す。外部処理そのものの停止完了までを1秒以内とはしない。

LLM latencyとTask所要時間はこのmilestoneではhard gateにせずbaselineを記録する。測定環境、手順、sample数、結果のばらつきを残し、次のRelease計画でbaselineを基にGateを更新する。

## 完了条件

- Support Matrixの両OSと言語で、該当する代表シナリオが再現可能な手順により完了する。
- 最低性能Gateを満たす。
- 失敗した検証は、再現条件、dataへの影響、回避策、次の判断とともに記録される。
- 性能baselineと既知の制約が記録される。
- 要件を満たすための未実装範囲が、このmilestoneの完了によって完了扱いにならない。
