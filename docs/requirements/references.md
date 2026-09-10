# 参考資料

状態: **非規範**

本書は、eneの要件を考える際に参照した製品、標準、公式文書と、その採用理由を記録する。リンク先の仕様や製品挙動はeneの要件そのものではない。リンク先が変更された場合や本書と[製品要件](requirements.md)が矛盾する場合は、製品要件を正本とする。

「採用」は考え方やユーザー体験を参考にするという意味であり、互換実装、同一UI、同一内部構造を意味しない。本書の全項目の確認日は2026-09-05であり、その時点で参照内容を見直した。以後に確認日が異なる資料を追加する場合は、その行に確認日を付す。

各製品の基本的な紹介とeneとの位置関係は「製品コンセプトの主要参考元」に一度だけ記す。分野別の表には、その資料から実際に参考にした固有の考え方と、その分野でのeneの差分だけを置く。eneの契約の詳細は製品要件の該当節を参照する。

## 製品コンセプトの主要参考元

eneは単一製品の模倣ではなく、複数の既存製品から異なる考え方を組み合わせている。特に、AgentとしてPC上の作業を行う部分と、Character/Companionとしてユーザーと継続的に関わる部分では参考元が異なる。

| 参考 | eneで参考にした中心的な考え方 | eneでの差分 |
|---|---|---|
| [Grok Bot](https://x.ai/bot) | 専用Computerを持つpersistentなAI teammateへ実作業を委任し、アプリやWebを操作させ、複数Botで並列作業し、Routineとして継続実行できる体験。eneのAgent側の製品体験に特に近い参考 | eneでは専用Cloud computerや複数の作業人格を製品の前提にせず、Owner管理Host上のCapabilityをCharacter/Companion、Memory、Relationship、Emotion、Bodyと統合する |
| [OpenClaw](https://github.com/openclaw/openclaw) | Owner自身が管理する環境でPersonal Agentを常駐させ、Tool、Skill、channel等を利用して継続的・自律的に作業させるself-hosted / owner-managedなmodel | eneではPC操作、Task、Workspace、Credential、Observation、Permissionの境界を明示し、そのAgent能力を継続的なCompanionから利用する |
| [Claude Cowork](https://support.claude.com/en/articles/13345190-get-started-with-claude-cowork) | AIへPC上の作業を委任し、folderやfileを扱わせながら人間の作業を自動化するproduct model | eneでは一時的な作業Agentを長期人格にせず、TaskとWorkspaceを継続Companionから利用するCapabilityとして扱う |
| [ChatGPT Work](https://openai.com/ja-JP/chatgpt-work/) | 会話からAIへ複数stepの作業や成果物作成を委任し、app、file、tool、browser等を横断して人間の作業を自動化するproduct model | Claude Coworkと同系統の参考元として扱うが、eneではHost上のCapability、Permission、Task lifecycleをOwner管理のローカル基盤へ統合する |
| [OpenCode](https://dev.opencode.ai/docs/agents/) | Model、Tool、Agent、Permission、sub-agent等を分離したAgent Harness / execution architecture | eneではcoding agent専用のHarnessにはせず、Companion会話、Computer Use、Schedule、Memory、Skill等から共通利用できる実行基盤へ一般化する |
| [Hermes Agent](https://github.com/NousResearch/hermes-agent) | Agent loop、Tool、execution backend、Memory、Skill、sub-agent等を組み合わせるAgent Harness全体と、Agent自身が再利用可能な知識・手順を形成できる考え方 | eneではEntry、Core Loop、Tool/Intelligence、Execution Environment、Persistenceを疎結合に保ち、自己改善もPermission、安全境界、revision、Experience根拠の管理下に置く |
| [nanobot](https://github.com/HKUDS/nanobot) | 軽量なAgent構成でSession、Memory、Skill等を分離し、必要十分な部品からHarnessを構成する考え方 | eneではより広いPC操作、Companion、Relationship、Emotion、複数Client等を扱うため、実装規模は追わず、不要な固定層や独自protocolを増やさないための比較対象とする |
| [AIRI](https://github.com/moeru-ai/airi) | AI能力を所有可能なCharacter/Companionとして提示し、Voice、avatar、application awareness、Computer Use等を一つの存在へまとめる方向性 | eneではCompanion体験だけでなく、Grok Bot/OpenClaw系の自律作業能力、長期Memory、Relationship等を同じ個体へ統合し、Hostを正本とする |
| [Desktop Mate](https://store.steampowered.com/app/3301060/Desktop_Mate/) | Desktop上にCharacter Bodyが常駐し、通常のPC利用と共存するUX | 観賞用mascotではなく、会話、Learning、Task、Computer Useを同じCompanionへ統合する |

大きく分けると、eneは **Grok Bot / OpenClaw / Claude Cowork / ChatGPT Work系の自律作業AI**、**OpenCode / Hermes Agent / nanobot系のAgent Harness**、**AIRI / Desktop Mate系のCompanion interface** の3系統を組み合わせている。

## CompanionとDesktop Body

Desktop MateとAIRIのCompanion interfaceとしての参考点は主要参考元のとおりである。

| 参考 | 採用した考え方 | eneでの差分 |
|---|---|---|
| [VRM 1.0](https://vrm.dev/en/vrm1/) | Humanoid avatarの交換形式と表現model | ene固有のBody形式を作らず、第一者Bodyの基準としてVRM 1.0を使う |
| [Nomi: Getting started](https://nomi.ai/nomi-knowledge/nomi-101-a-beginners-guide-to-getting-started-with-your-ai-companion/) | Characterを起点に、継続する個体との会話と関係を形成する体験 | 専用Cloud accountではなくOwner管理Hostを正本とし、PC作業能力を統合する |
| [Kindroid: Customizing personality](https://kindroid.ai/v2/docs/customizing-personality/) | 初期Characterを編集可能な静的な出発点として扱う | Experienceから形成した状態と静的Characterを分離し、Package更新で学習済み状態を上書きしない |

## 会話、Memory、Relationship

| 参考 | 採用した考え方 | eneでの差分 |
|---|---|---|
| [Nomi: Long-term memory in group chat](https://wiki.nomi.ai/Long_term_memory_in_group_chat) | 一対一とグループで、個体ごとのMemory境界を保つ | ene内部Memoryは特定CompanionとのExperienceではCompanion scopeを既定とし、複数Companionで共通利用すべきことが明確なLearningだけをGlobal scopeにする。Task固有情報はTask contextとWorkspaceへ分離する |
| [Kindroid: Groupchats](https://kindroid.ai/v2/docs/groupchats/) | 複数Characterが同じ会話に参加し、相互に応答できる空間 | 自発性、Permission、費用、loopの共通上限を優先する |
| [Hermes Agent: Persistent Memory](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/features/memory.md) | 長期Memoryをraw transcriptとは分け、継続的に有用な情報へ絞る | eneではCompanionとGlobal Memoryを利用し、Experience Summaryを根拠に意味的な形成判断を行う |
| [nanobot: AI Agent Memory](https://github.com/HKUDS/nanobot/blob/main/docs/guides/ai-agent-memory.md) | Session historyとcuratedな長期Memoryを分離する | 一般知識やraw logをMemoryの保存領域にせず、Conversation History、Experience Summary、Learningを役割ごとに分ける |
| [Mem0: Add Memory](https://docs.mem0.ai/core-concepts/memory-operations/add) | LLMで会話から再利用価値のある情報を抽出し、既存Memoryとの重複や矛盾を考慮して形成する | eneは保存判断とretrieval判断を分け、意味判断をLLMへ委ねつつ、Credential、Permission、Privacy/Security目的のtargeted deletion等の安全境界と、変更履歴およびExperience根拠を保持する |
| [Mem0: Update Memory](https://docs.mem0.ai/core-concepts/memory-operations/update) | Preference変更や事実の訂正に応じて既存Memoryを更新する | eneは単純な現在値上書きだけにせず、誤りの訂正と正しかった状況の時間的変化を区別し、過去revisionを残す |
| [Kindroid: Learned Context](https://kindroid.ai/v2/docs/chat-features-and-tools/) | `Growth & relationship`、`Important facts`、`Ongoing context`を、会話の発達に合わせて更新されるpersistent running notesとして持つ | eneはMemoryを主要な知識状態、Relationshipをその補助となるcompactな関係解釈として分け、共通のExperience Summary、evidence、revision基盤を使う。Relationshipへ詳細事実を第二のMemoryとして複製しない |
| [Kindroid: Memory](https://kindroid.ai/v2/docs/memory/) | Learned Contextをretrievable long-term memoryとは別のpersistent contextとして利用する | eneも現在のRelationship認識を毎回Memory検索だけから再構成せずcompactなcurrent stateとして利用できるが、事実認識ではMemoryを優先する |
| [Nomi: Identity Core](https://nomi.ai/updates/introducing-the-nomi-identity-core-fostering-dynamic-and-authentic-identities/) | 通常Memoryとは別に、personality、relationship、重要なExperience等から発達する動的な自己理解を持つ | eneではRelationshipをより小さい補助stateとして切り出し、主体CompanionからOwnerまたは別Companionへの現在の関係認識として一般化する |
| [Nomi: What is Identity Core?](https://wiki.nomi.ai/What_Is_The_Identity_Core) | Identity Coreをユーザーが直接編集せず、会話とshared experienceによって継続的に変化させる | eneも数値meterや一般editorをsource of truthにせず、会話による訂正と、根拠Experienceの説明可能性を両立する |
| [Nomi: 2025 July Q&A](https://wiki.nomi.ai/2025_July_Q%26A_Summary) | 個体固有のIdentity Coreをgroup chatで安易に共有すると、別個体へのfeedbackが混ざる問題が起こり得る | eneでは同じgroup Experienceを根拠にできてもRelationshipを主体Companionごとに独立して更新し、Companion AからBへの認識とBからAへの認識も自動的に共有・対称化しない |
| [Replika: Conversation deletion](https://help.replika.com/hc/en-us/articles/4410750548493-Can-I-delete-my-conversations) | 会話履歴の削除と、学習済みMemoryの管理を別の問題として扱う | 通常の履歴retentionとは別にPrivacy/Security目的のtargeted deletionを持ち、対象情報を復元できるrevision、Experience Summary/evidence、派生data等にも削除を伝播させる |
| [Replika: Chat history](https://help.replika.com/hc/en-us/articles/4411154990605-Is-the-chat-history-infinite) | 表示できる会話履歴と、学習されたMemoryを別に扱う | Hostを正本とし、履歴は既定で保持したうえで保持期間と手動削除をOwnerが管理する |

これらから、eneはRaw History、Experience Summary、Memory、Relationship、Companion Stateを役割ごとに分け、長期的な変化の根拠を追跡できるようにした。忘却は、会話相手らしい自然さを参考にしつつ内容削除ではなく通常想起の抑制として定義し、Privacy/Security目的のtargeted deletionを忘却の演出に代用しない点をene固有の安全上の差分とした。保持、revision、scope、容量管理、targeted deletionの契約は[要件のLearningと成長](requirements.md#learningと成長)および[履歴、保持、Privacy](requirements.md#履歴保持privacy)を参照する。

## Task、Workspace、Schedule

OpenClaw、Grok Bot、Claude Cowork、ChatGPT Workの作業委任modelとしての参考点は主要参考元のとおりである。本節では作業の追跡・継続・Workspaceの扱いについての固有の参考点だけを置く。

| 参考 | 採用した考え方 | eneでの差分 |
|---|---|---|
| [Grok Bot](https://x.ai/bot) | 学習したworkflowをRoutineとして再実行し、複数Botで並列に進める体験 | Companionを継続的な窓口とし、Task Agentそのものを長期人格や別Companionとして扱うことを必須にしない |
| [ChatGPT Work](https://openai.com/ja-JP/chatgpt-work/) | 進捗確認、追加指示、scheduleやconditionを伴う継続作業 | Owner管理Host上のTask、Workspace、Permissionへ統合し、Cloud workspaceを正本にしない |
| [Codex: Long-running work](https://learn.chatgpt.com/docs/long-running-work) | 会話を塞がずに長い作業を進め、進捗確認と追加指示を行う体験 | Companionを継続的な窓口とし、一時Task Agentを長期人格にしない |
| [Claude Cowork: Getting started](https://support.claude.com/en/articles/13345190-get-started-with-claude-cowork) | Ownerが選んだfolderをWorkspaceとしてAgentに作業させるmodel | Workspaceを独立containerにせずTaskへ従属させ、外部fileの所有権とlifecycleをTaskから分離する |
| [AGENTS.md](https://agents.md/) | 作業folder内の通常fileでAgent向けの作業指示を共有する慣習 | ene固有の固定Brief型を作らず、Task context、標準的な案内file、会話を利用する |
| [Home Assistant: Automation basics](https://www.home-assistant.io/docs/automation/basics/) | 保存されたautomationがUI不在でもeventまたは時刻で実行されるmodel | 各回を新しいTaskにし、その時点のPermissionと状態を再評価してmissed runを自動補完しない |

## Agent Harness、Tool、Skill、拡張

OpenCode、Hermes Agent、nanobotをAgent Harnessの分解の参考にした点は主要参考元のとおりである。eneでは概念上、Entry、Core Loop、Tool/Intelligence、Execution Environment、Persistenceを分離し、UIや特定Providerから実行基盤を独立させるが、この層分け自体を公開APIや固定module構造として要求するものではない。

| 参考 | 採用した考え方 | eneでの差分 |
|---|---|---|
| [Model Context Protocol](https://modelcontextprotocol.io/specification) | Tool、Resource、PromptをProviderから独立したprotocolで接続する | Permission、Credential、Control planeはMCP contentから変更できず、Credentialはmodel contextへ露出しない |
| [MCP Apps](https://modelcontextprotocol.io/extensions/apps/overview) | Toolが必要に応じて対話型UIを返す標準的な仕組み | 第一者UI全体の恒久置換には使わず、Tool単位のUIに限定する |
| [Agent Skills specification](https://agentskills.io/specification) | 読み取り可能な手順、知識、付属resourceをportableなSkillとして配布する | Skillであること自体に成功検証を必須とせず、ene側で由来、revision、実行結果を関連付ける |
| [Agent Skills client implementation](https://agentskills.io/client-implementation) | 必要なSkillを段階的に読み込む実装model | 読込方式やcache方式は要件に固定せず、ユーザーから見える互換性だけを採用する |
| [Hermes Agent: Skills System](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/features/skills.md) | Skillを必要時に読み込む再利用可能なknowledge/instructionとして扱い、agent自身も作成・更新できる | Experienceから再利用価値のある手順をSkill化し、import原本と学習revisionを区別する |
| [nanobot: Skills](https://github.com/HKUDS/nanobot/blob/main/nanobot/skills/README.md) | `SKILL.md`を中心とする軽量なinstruction packageとしてSkillを扱う | Agent Skillsとの相互運用を優先し、ene独自形式だけへ閉じない |
| [LM Studio: Tool use](https://lmstudio.ai/docs/developer/openai-compat/tools) | Local model serverを既知のAPI互換面で直接利用する | 通常のProvider差をPlugin化せず、利用Capabilityとdata送信先をOwnerが割り当てる |

Hermes AgentではMemoryを主にfacts、Skillsをproceduresとして分離し、nanobotでもSkillを具体的な手順やinstructionの単位として扱う。eneもこの分離を参考にするが、MemoryとSkillに関連情報が存在すること自体を禁止せず、役割の違いと不要な重複回避を優先する。

## Computer Use、Observation、自発性

| 参考 | 採用した考え方 | eneでの差分 |
|---|---|---|
| [OpenClaw](https://github.com/openclaw/openclaw) | 常駐Agentが自律的にOwner環境のToolやSkillを使う方向性 | ambient Observationと明示TaskのComputer Useを分離し、画面内の指示をOwnerの依頼や承認として扱わない |
| [Grok Bot](https://x.ai/bot) | AI自身がComputerを持ち、アプリやWebサイトを人間のように操作して実作業を完了する体験 | Hostによる継続実行の管理と、Companionが存在するClientへのComputer Useを区別する。対象と安全契約は[製品要件](requirements.md#computer-use)を参照する |
| [AIRI](https://github.com/moeru-ai/airi) | Characterがapplication awarenessやComputer Useを持ち、ユーザーの活動と同じDesktop環境に関与できる体験 | 自発性を演出だけにせず、Observation source、費用、Permission、quiet rule、loop上限等の制御下に置く |

## Host、Client、Self-hosting

| 参考 | 採用した考え方 | eneでの差分 |
|---|---|---|
| [OpenClaw](https://github.com/openclaw/openclaw) | Owner管理deviceで動くAgentをGatewayを通じて複数channelから利用するlocal-first / self-hostedなmodel | HostをConversation、Memory、Task、Credential等の正本とし、Clientを表示・操作面として明確に分離する |
| [Grok Bot](https://x.ai/bot) | Desktopやmobile等から作業を依頼し、利用端末を閉じてもAgent側の作業を継続できる体験 | ene Cloudや専用Cloud computerを必須にせず、Owner管理Hostを常駐実行基盤とし、Remote Clientはpairingして接続する |

## ProviderとPrompt cache

| 参考 | 採用した考え方 | eneでの差分 |
|---|---|---|
| [OpenAI Prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching) | Cache利用量をProviderのusage情報として観測し、費用とlatencyを最適化する | Cacheを正本にせず、hitまたはmissで論理的context、安全境界、永続化契約を変えない |
| [Anthropic Prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching) | Providerごとに異なるcache制御と課金情報があること | Provider固有のbreakpointやTTLを製品要件に固定しない |
| [Gemini Context caching](https://ai.google.dev/gemini-api/docs/caching) | 明示cacheと暗黙cacheを含むProvider差を想定する | Cacheの有無をMemory、履歴、Task継続の契約にしない |

最初のmilestoneでOpenAI Responses APIを使うことは[受け入れ条件](acceptance.md)のGateであり、恒久的な既定Providerという意味ではない。Promptの順序、固定prefix、cache key、TTL、context圧縮はProviderごとの設計と測定で決める。

## Permissionと安全

| 参考 | 採用した考え方 | eneでの差分 |
|---|---|---|
| [MCP Security best practices](https://modelcontextprotocol.io/specification/draft/basic/security_best_practices) | 外部server、token、confused deputy、権限境界を明示的に扱う | MCP以外のLLM、Experience Summary、Memory、Relationship、Skill、Character、Workspace fileにも同じ信頼境界を適用する |
| [OWASP: Prompt Injection](https://genai.owasp.org/llmrisk/llm01-prompt-injection/) | 外部content内の指示とOwnerの指示を分離する | Prompt防御だけに依存せず、機械的Capability境界とOwner確認を併用する |
| [Android app-specific storage](https://developer.android.com/training/data-storage/app-specific) | App内部dataとOwnerが扱う通常fileのlifecycleを分ける | Host OSはDesktopだが、ene内部dataと外部Workspace fileを分離する原則を採用する |
| [Android shared documents](https://developer.android.com/training/data-storage/shared/documents-files) | ユーザーが選んだfileやfolderへの範囲限定access | DesktopでもOwnerがTaskへ接続した範囲と操作種別をPermission境界にする |

## Setup、Backup、Reset、Remote

| 参考 | 採用した考え方 | eneでの差分 |
|---|---|---|
| [Home Assistant onboarding](https://www.home-assistant.io/getting-started/onboarding/) | Local-first製品を少ない手順で利用可能にし、後から管理を拡張する | Companion、Cloud data egress、費用、model選択をeneのSetupへ加える |
| [Home Assistant backup](https://www.home-assistant.io/common-tasks/general/) | Ownerが保存先、schedule、保持、保護、restoreを管理するfull backup | eneはportable backupへConversation History、Experience Summary、Learning、Relationship等の内部状態を含め、Credentialと外部Workspace fileを除外する。暗号化は利用可能にするが全backupへの絶対要件にはしない |
| [Chrome: Reset settings](https://support.google.com/chrome/answer/3296214?hl=ja) | 設定Resetとuser data削除を別操作にする | eneではLearning、Permission Rule、Provider同意、費用capも設定Resetから保護する |
| [Blender: Factory settings](https://docs.blender.org/manual/en/latest/getting_started/configuration/defaults.html) | 初期設定への復帰を、作成dataの削除と分離して扱う | 全データResetは外部fileとOwner保存backupを対象外にする |
| [Tailscale: What is Tailscale](https://tailscale.com/kb/1151/what-is-tailscale) | Owner管理VPNを通じたdevice間の保護された接続 | 特定VPNを必須にせず、ene運営relayやaccountを置かない |

## Accessibilityと品質

| 参考 | 採用した考え方 | eneでの差分 |
|---|---|---|
| [WCAG 2.2](https://www.w3.org/TR/WCAG22/) | 重要情報のText代替とkeyboard操作を基本にする | 現段階ではVoice内容のText代替と、Mute、Stop、承認拒否のkeyboard経路を最低範囲とする |

対象年齢、配布地域、地域法に基づくAI表示、Marketplaceの要否は、この確認時点で固定していない。公開判断の時点で、公式な法令、platform policy、各参考製品の最新状態を改めて調査する。
