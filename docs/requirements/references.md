# 参考資料

状態: **非規範**
最終確認: 2026-09-05

本書は、Eneの要件を考える際に参照した製品、標準、公式文書と、その採用理由を記録する。リンク先の仕様や製品挙動はEneの要件そのものではない。リンク先が変更された場合や本書と[製品要件](requirements.md)が矛盾する場合は、製品要件を正本とする。

「採用」は考え方やユーザー体験を参考にするという意味であり、互換実装、同一UI、同一内部構造を意味しない。確認日は、その時点で参照内容を見直した日である。

## 製品コンセプトの主要参考元

Eneは単一製品の模倣ではなく、複数の既存製品から異なる考え方を組み合わせている。特に、AgentとしてPC上の作業を行う部分と、Character/Companionとしてユーザーと継続的に関わる部分では参考元が異なる。

| 参考 | Eneで参考にした中心的な考え方 | Eneでの差分 | 確認日 |
|---|---|---|---|
| OpenClaw | セルフホストしたAIがOwnerのPC環境へ比較的自由にアクセスし、Tool、Computer Use、schedule等を利用しながら自律的に作業するという、EneのNon-Companion Agent System全体の原型 | EneではこのAgent能力を独立したbot体験として前面に出さず、Permission、Task、Workspace、Credential、観測境界を明示し、継続的なCompanionを主な窓口として統合する | 2026-09-05 |
| Grok Bot | 常駐するAIと継続的に会話しながら、PC操作、Computer Use、Task、自律的な作業を依頼できる統合体験。OpenClawと並び、Ene全体の原型に近い参考元 | EneではCharacter/Companion、Memory、Relationship、Emotion、Body等を第一級の概念として統合し、Agent機能だけでなく長期的な個体との関係を製品の中心に置く | 2026-09-05 |
| Claude Cowork | AIへPC上の作業を委任し、folderやfileを扱わせながら人間の作業を自動化するproduct model | Eneでは一時的な作業Agentを長期人格にせず、TaskとWorkspaceを継続Companionから利用するCapabilityとして扱う | 2026-09-05 |
| ChatGPT Work | 会話からAIへPC作業や成果物作成を委任し、人間の作業をAgentへ移すproduct model | Claude Coworkと同系統の参考元として扱うが、EneではHost上のCapability、Permission、Task lifecycleをOwner管理のローカル基盤へ統合する | 2026-09-05 |
| OpenCode | Provider、Model、Tool、Session、Permission、execution environment等を分離したAgent Harness / execution architecture | Eneではcoding agent専用のHarnessにはせず、Companion会話、Computer Use、Schedule、Memory、Skill等から共通利用できる実行基盤へ一般化する | 2026-09-05 |
| Hermes Agent | Agent loop、Tool、execution backend、Memory、Skill、sub-agent等を組み合わせるAgent Harness全体と、Agent自身が再利用可能な知識・手順を形成できる考え方 | EneではEntry、Core Loop、Tool/Intelligence、Execution Environment、Persistenceを疎結合に保ち、自己改善もPermission、安全境界、revision、Experience根拠の管理下に置く | 2026-09-05 |
| nanobot | 軽量なAgent構成でSession、Memory、Skill等を分離し、必要十分な部品からHarnessを構成する考え方 | Eneではより広いPC操作、Companion、Relationship、Emotion、複数Client等を扱うため、そのままの軽量構成ではなく概念上の参考とする | 2026-09-05 |
| AIRI | AI Agent能力をCharacter/Companionとして提示し、Voice、avatar、継続的な会話、感情、Memory等を一つの存在へまとめる方向性 | EneではCompanion表示だけでなく、OpenClaw/Grok Bot系の自律作業能力を同じ個体へ統合し、Hostを正本とした長期状態を持つ | 2026-09-05 |
| Desktop Mate | Desktop上にCharacter Bodyが常駐し、通常のPC利用と共存するUX | 観賞用mascotではなく、会話、Learning、Task、Computer Useを同じCompanionへ統合する | 2026-09-05 |

大きく分けると、Eneは **OpenClaw / Grok Bot / Claude Cowork / ChatGPT Work系の自律作業AI**、**OpenCode / Hermes Agent / nanobot系のAgent Harness**、**AIRI / Desktop Mate系のCompanion interface** の3系統を組み合わせている。OpenClawとGrok Botは特に個別機能だけの参考ではなく、Eneが「セルフホストされたAIがPC上で自律的に作業し、それを継続的な対話から利用できる」製品であるという原型に近い。

## CompanionとDesktop Body

| 参考 | 採用した考え方 | Eneでの差分 | 確認日 |
|---|---|---|---|
| [Desktop Mate](https://store.steampowered.com/app/3301060/Desktop_Mate/) | Desktop上に常駐し、通常のPC利用と共存するCharacter Body | 観賞用mascotではなく、会話、Learning、Taskを同じCompanionへ統合する | 2026-09-05 |
| AIRI | Character、Voice、avatar、感情、Memory等を統合したAI companion / AITuber型の体験 | Agent能力を裏側の別製品として分離せず、同じCompanionが会話とPC作業の両方を担う | 2026-09-05 |
| [VRM 1.0](https://vrm.dev/en/vrm1/) | Humanoid avatarの交換形式と表現model | Ene固有のBody形式を作らず、第一者Bodyの基準としてVRM 1.0を使う | 2026-09-05 |
| [Nomi: Getting started](https://nomi.ai/nomi-knowledge/nomi-101-a-beginners-guide-to-getting-started-with-your-ai-companion/) | Characterを起点に、継続する個体との会話と関係を形成する体験 | 専用Cloud accountではなくOwner管理Hostを正本とし、PC作業能力を統合する | 2026-09-05 |
| [Kindroid: Customizing personality](https://kindroid.ai/v2/docs/customizing-personality/) | 初期Characterを編集可能な静的な出発点として扱う | Experienceから形成した状態と静的Characterを分離し、Package更新で学習済み状態を上書きしない | 2026-09-05 |

## 会話、Memory、Relationship

| 参考 | 採用した考え方 | Eneでの差分 | 確認日 |
|---|---|---|---|
| AIRI | Characterが継続的なExperienceを持ち、Memory、感情、personalityの変化を通じて長期的に振る舞いが形成される方向性 | EneではRaw History、Experience Summary、Memory、Relationship、Companion Stateを分け、時間減衰や形成判断の詳細を固定実装せず要件上の責務として分離する | 2026-09-05 |
| [Nomi: Long-term memory in group chat](https://wiki.nomi.ai/Long_term_memory_in_group_chat) | 一対一とグループで、個体ごとのMemory境界を保つ | Ene内部Memoryは特定CompanionとのExperienceではCompanion scopeを既定とし、複数Companionで共通利用すべきことが明確なLearningだけをGlobal scopeにする。Task固有情報はTask contextとWorkspaceへ分離する | 2026-09-05 |
| [Kindroid: Groupchats](https://kindroid.ai/v2/docs/groupchats/) | 複数Characterが同じ会話に参加し、相互に応答できる空間 | 自発性、Permission、費用、loopの共通上限を優先する | 2026-09-05 |
| [Hermes Agent: Persistent Memory](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/features/memory.md) | 長期Memoryをraw transcriptとは分け、継続的に有用な情報へ絞る | EneではCompanionとGlobal Memoryを利用し、Experience Summaryを根拠に意味的な形成判断を行う | 2026-09-05 |
| [nanobot: AI Agent Memory](https://github.com/HKUDS/nanobot/blob/main/docs/guides/ai-agent-memory.md) | Session historyとcuratedな長期Memoryを分離する | 一般知識やraw logをMemoryの保存領域にせず、Conversation History、Experience Summary、Learningを役割ごとに分ける | 2026-09-05 |
| [Mem0: Add Memory](https://docs.mem0.ai/core-concepts/memory-operations/add) | LLMで会話から再利用価値のある情報を抽出し、既存Memoryとの重複や矛盾を考慮して形成する | Eneは保存判断とretrieval判断を分け、意味判断をLLMへ委ねつつ、Credential、Permission、Privacy/Security目的のtargeted deletion等の安全境界と、変更履歴およびExperience根拠を保持する | 2026-09-05 |
| [Mem0: Update Memory](https://docs.mem0.ai/core-concepts/memory-operations/update) | Preference変更や事実の訂正に応じて既存Memoryを更新する | Eneは単純な現在値上書きだけにせず、誤りの訂正と正しかった状況の時間的変化を区別し、過去revisionを残す | 2026-09-05 |
| [Kindroid: Learned Context](https://kindroid.ai/v2/docs/chat-features-and-tools/) | `Growth & relationship`、`Important facts`、`Ongoing context`を、会話の発達に合わせて更新されるpersistent running notesとして持つ | EneはMemoryを主要な知識状態、Relationshipをその補助となるcompactな関係解釈として分け、共通のExperience Summary、evidence、revision基盤を使う。Relationshipへ詳細事実を第二のMemoryとして複製しない | 2026-09-05 |
| [Kindroid: Memory](https://kindroid.ai/v2/docs/memory/) | Learned Contextをretrievable long-term memoryとは別のpersistent contextとして利用する | Eneも現在のRelationship認識を毎回Memory検索だけから再構成せずcompactなcurrent stateとして利用できるが、事実認識ではMemoryを優先する | 2026-09-05 |
| [Nomi: Identity Core](https://nomi.ai/updates/introducing-the-nomi-identity-core-fostering-dynamic-and-authentic-identities/) | 通常Memoryとは別に、personality、relationship、重要なExperience等から発達する動的な自己理解を持つ | EneではRelationshipをより小さい補助stateとして切り出し、主体CompanionからOwnerまたは別Companionへの現在の関係認識として一般化する | 2026-09-05 |
| [Nomi: What is Identity Core?](https://wiki.nomi.ai/What_Is_The_Identity_Core) | Identity Coreをユーザーが直接編集せず、会話とshared experienceによって継続的に変化させる | Eneも数値meterや一般editorをsource of truthにせず、会話による訂正と、根拠Experienceの説明可能性を両立する | 2026-09-05 |
| [Nomi: 2025 July Q&A](https://wiki.nomi.ai/2025_July_Q%26A_Summary) | 個体固有のIdentity Coreをgroup chatで安易に共有すると、別個体へのfeedbackが混ざる問題が起こり得る | Eneでは同じgroup Experienceを根拠にできてもRelationshipを主体Companionごとに独立して更新し、Companion AからBへの認識とBからAへの認識も自動的に共有・対称化しない | 2026-09-05 |
| [Replika: Conversation deletion](https://help.replika.com/hc/en-us/articles/4410750548493-Can-I-delete-my-conversations) | 会話履歴の削除と、学習済みMemoryの管理を別の問題として扱う | 通常の履歴retentionとは別にPrivacy/Security目的のtargeted deletionを持ち、対象情報を復元できるrevision、Experience Summary/evidence、派生data等にも削除を伝播させる | 2026-09-05 |
| [Replika: Chat history](https://help.replika.com/hc/en-us/articles/4411154990605-Is-the-chat-history-infinite) | 表示できる会話履歴と、学習されたMemoryを別に扱う | Hostを正本とし、履歴は既定で保持したうえで保持期間と手動削除をOwnerが管理する | 2026-09-05 |

Eneでは、Raw History、意味的なまとまりへ圧縮したExperience Summary、そこから形成されるMemoryとRelationshipを分ける。Memory形成時の保存価値と、後の会話でのretrieval優先度は別の判断とし、Memoryは内容、重要度、scope、時間的意味等をExperienceに応じて継続更新できる一方、過去revisionと根拠を保持する。通常の忘却、訂正、失効、置換、統合は保存済みMemoryや過去revision・根拠の削除を意味しない。Privacy/Security目的で特定情報そのものの強制消去を明示したtargeted deletionだけがこの保持原則より優先し、対象情報を復元できるrevision、Experience Summary/evidence、派生data等も削除対象にする。

特定CompanionとのExperienceから形成されたLearningはCompanion scopeを既定とする。Global scopeはOwnerが明示的に共有を求めた場合、または複数Companionで共通に利用すべきことが内容と文脈から明確な場合に限り、単に重要、有用、一般的な好みであることだけをGlobal化の理由にしない。

Relationshipは同じExperience/evidence/revision基盤を利用できるが、Memoryより優先度の低い補助状態であり、詳細な事実や出来事を複製せず関係そのもののcompactな現在解釈へ絞る。Relationshipの相手はOwnerだけに限定せず別Companionも含め、各Companion自身の認識として独立に更新する。

Eneの忘却は、会話相手らしい自然さを参考にしつつ、内容削除ではなく通常想起の抑制として定義した。「忘れてほしい」等の通常依頼も、Privacy/Security目的で保存済み情報そのものの消去を明示しない限りこの通常忘却として扱う。重要度とscopeは別の概念であり、重要度だけを理由にGlobal scopeへ昇格させない。Privacy/Security目的のtargeted deletionを忘却の演出に代用しない点はEne固有の安全上の差分である。

## Task、Workspace、Schedule

| 参考 | 採用した考え方 | Eneでの差分 | 確認日 |
|---|---|---|---|
| OpenClaw | Self-hosted AgentがOwnerのPC環境、Tool、schedule等を使って、自律的に継続作業するmodel | 作業はTaskとして追跡し、Permission、Credential、Workspace、loop上限等の共通境界を適用する | 2026-09-05 |
| Grok Bot | 継続的な会話からComputer UseやTaskを依頼し、会話とは別にAgentが作業を進められる体験 | Companionを継続的な窓口とし、Task Agentそのものを長期人格や別Companionとして扱わない | 2026-09-05 |
| ChatGPT Work | AIへPC上の作業や成果物作成を委任し、作業中も追加指示や結果確認を行う体験 | Owner管理Host上のTask、Workspace、Permissionへ統合し、Cloud workspaceを正本にしない | 2026-09-05 |
| [Codex: Long-running work](https://learn.chatgpt.com/docs/long-running-work) | 会話を塞がずに長い作業を進め、進捗確認と追加指示を行う体験 | Companionを継続的な窓口とし、一時Task Agentを長期人格にしない | 2026-09-05 |
| [Claude Cowork: Getting started](https://support.claude.com/en/articles/13345190-get-started-with-claude-cowork) | AIへPC上の作業を委任し、Ownerが選んだfolderをWorkspaceとしてAgentに作業させるmodel | Workspaceを独立containerにせずTaskへ従属させ、外部fileの所有権とlifecycleをTaskから分離する | 2026-09-05 |
| [AGENTS.md](https://agents.md/) | 作業folder内の通常fileでAgent向けの作業指示を共有する慣習 | Ene固有の固定Brief型を作らず、Task context、標準的な案内file、会話を利用する | 2026-09-05 |
| [Home Assistant: Automation basics](https://www.home-assistant.io/docs/automation/basics/) | 保存されたautomationがUI不在でもeventまたは時刻で実行されるmodel | 各回を新しいTaskにし、その時点のPermissionと状態を再評価してmissed runを自動補完しない | 2026-09-05 |

## Agent Harness、Tool、Skill、拡張

OpenCode、Hermes Agent、nanobotは個別機能だけでなく、EneのAgent Harnessを分解して考える際の参考元でもある。Eneでは概念上、Entry、Core Loop、Tool/Intelligence、Execution Environment、Persistenceを分離し、UIや特定Providerから実行基盤を独立させる。ただし、この層分け自体を公開APIや固定module構造として要求するものではない。

| 参考 | 採用した考え方 | Eneでの差分 | 確認日 |
|---|---|---|---|
| OpenCode | Provider/Model abstraction、Tool execution、Session/Message/Event、Permission/Approval等を分離したAgent execution architecture | coding agent専用ではなく、会話、Task、Computer Use、Scheduleから共通利用できるHarnessとして一般化する | 2026-09-05 |
| Hermes Agent | Agent loop、Tool、execution backend、Memory、Skill、sub-agent等を組み合わせ、再利用可能なAgent能力を構成するHarness | 各CapabilityをCompanionから独立した基盤として扱いつつ、Permission、Credential、revision、Experience等のEne固有境界を適用する | 2026-09-05 |
| nanobot | Session、Memory、Skill、Tool等を小さく分離した軽量なAgent構成 | 実装規模そのものは追わず、不要な固定層や独自protocolを増やさないための比較対象として利用する | 2026-09-05 |
| [Model Context Protocol](https://modelcontextprotocol.io/specification) | Tool、Resource、PromptをProviderから独立したprotocolで接続する | Permission、Credential、Control planeはMCP contentから変更できず、Credentialはmodel contextへ露出しない | 2026-09-05 |
| [MCP Apps](https://modelcontextprotocol.io/extensions/apps/overview) | Toolが必要に応じて対話型UIを返す標準的な仕組み | 第一者UI全体の恒久置換には使わず、Tool単位のUIに限定する | 2026-09-05 |
| [Agent Skills specification](https://agentskills.io/specification) | 読み取り可能な手順、知識、付属resourceをportableなSkillとして配布する | Skillであること自体に成功検証を必須とせず、Ene側で由来、revision、実行結果を関連付ける | 2026-09-05 |
| [Agent Skills client implementation](https://agentskills.io/client-implementation) | 必要なSkillを段階的に読み込む実装model | 読込方式やcache方式は要件に固定せず、ユーザーから見える互換性だけを採用する | 2026-09-05 |
| [Hermes Agent: Skills System](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/features/skills.md) | Skillを必要時に読み込む再利用可能なknowledge/instructionとして扱い、agent自身も作成・更新できる | Experienceから再利用価値のある手順をSkill化し、import原本と学習revisionを区別する | 2026-09-05 |
| [nanobot: Skills](https://github.com/HKUDS/nanobot/blob/main/nanobot/skills/README.md) | `SKILL.md`を中心とする軽量なinstruction packageとしてSkillを扱う | Agent Skillsとの相互運用を優先し、Ene独自形式だけへ閉じない | 2026-09-05 |
