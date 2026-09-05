# 参考資料

状態: **非規範**
最終確認: 2026-09-05

本書は、Eneの要件を考える際に参照した製品、標準、公式文書と、その採用理由を記録する。リンク先の仕様や製品挙動はEneの要件そのものではない。リンク先が変更された場合や本書と[製品要件](requirements.md)が矛盾する場合は、製品要件を正本とする。

「採用」は考え方やユーザー体験を参考にするという意味であり、互換実装、同一UI、同一内部構造を意味しない。確認日は、その時点で参照内容を見直した日である。

## CompanionとDesktop Body

| 参考 | 採用した考え方 | Eneでの差分 | 確認日 |
|---|---|---|---|
| [Desktop Mate](https://store.steampowered.com/app/3301060/Desktop_Mate/) | Desktop上に常駐し、通常のPC利用と共存するCharacter Body | 観賞用mascotではなく、会話、Learning、Taskを同じCompanionへ統合する | 2026-09-05 |
| [VRM 1.0](https://vrm.dev/en/vrm1/) | Humanoid avatarの交換形式と表現model | Ene固有のBody形式を作らず、第一者Bodyの基準としてVRM 1.0を使う | 2026-09-05 |
| [Nomi: Getting started](https://nomi.ai/nomi-knowledge/nomi-101-a-beginners-guide-to-getting-started-with-your-ai-companion/) | Characterを起点に、継続する個体との会話と関係を形成する体験 | 専用Cloud accountではなくOwner管理Hostを正本とし、PC作業能力を統合する | 2026-09-05 |
| [Kindroid: Customizing personality](https://kindroid.ai/v2/docs/customizing-personality/) | 初期Characterを編集可能な静的な出発点として扱う | Experienceから形成した状態と静的Characterを分離し、Package更新で学習済み状態を上書きしない | 2026-09-05 |

## 会話、Memory、Relationship

| 参考 | 採用した考え方 | Eneでの差分 | 確認日 |
|---|---|---|---|
| [Nomi: Long-term memory in group chat](https://wiki.nomi.ai/Long_term_memory_in_group_chat) | 一対一とグループで、個体ごとのMemory境界を保つ | Ene内部MemoryはCompanionとGlobalのscopeを持ち、Task固有情報はTask contextとWorkspaceへ分離する | 2026-09-05 |
| [Kindroid: Groupchats](https://kindroid.ai/v2/docs/groupchats/) | 複数Characterが同じ会話に参加し、相互に応答できる空間 | 自発性、Permission、費用、loopの共通上限を優先する | 2026-09-05 |
| [Hermes Agent: Persistent Memory](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/features/memory.md) | 長期Memoryをraw transcriptとは分け、継続的に有用な情報へ絞る | Eneでは専用User Profileを設けず、CompanionとGlobal Memoryへ統合する。詳細な形成基準は別途定める | 2026-09-05 |
| [nanobot: AI Agent Memory](https://github.com/HKUDS/nanobot/blob/main/docs/guides/ai-agent-memory.md) | Session historyとcuratedな長期Memoryを分離する | 一般知識やraw logをMemoryの保存領域にせず、Conversation HistoryとLearningを別に扱う | 2026-09-05 |
| [Replika: Conversation deletion](https://help.replika.com/hc/en-us/articles/4410750548493-Can-I-delete-my-conversations) | 会話履歴の削除と、学習済みMemoryの管理を別の問題として扱う | Privacy/Security目的のtargeted deletionと、容量管理のlog retentionを分ける | 2026-09-05 |
| [Replika: Chat history](https://help.replika.com/hc/en-us/articles/4411154990605-Is-the-chat-history-infinite) | 表示できる会話履歴と、学習されたMemoryを別に扱う | Hostを正本とし、履歴は既定で保持したうえで保持期間と手動削除をOwnerが管理する | 2026-09-05 |

Eneの忘却は、会話相手らしい自然さを参考にしつつ、内容削除ではなく通常想起の抑制として定義した。重要度とscopeを分離し、Privacy削除を忘却の演出に代用しない点はEne固有の安全上の差分である。Memory形成基準とRelationship更新modelの詳細は、このBaselineとは分けて追加検討する。

## Task、Workspace、Schedule

| 参考 | 採用した考え方 | Eneでの差分 | 確認日 |
|---|---|---|---|
| [Codex: Long-running work](https://learn.chatgpt.com/docs/long-running-work) | 会話を塞がずに長い作業を進め、進捗確認と追加指示を行う体験 | Companionを継続的な窓口とし、一時Task Agentを長期人格にしない | 2026-09-05 |
| [Claude Cowork: Getting started](https://support.claude.com/en/articles/13345190-get-started-with-claude-cowork) | Ownerが選んだfolderをWorkspaceとしてAgentに作業させるmodel | Workspaceを独立ProjectにせずTaskへ従属させ、外部fileの所有権とlifecycleをTaskから分離する | 2026-09-05 |
| [AGENTS.md](https://agents.md/) | 作業folder内の通常fileでAgent向けの作業指示を共有する慣習 | Ene固有の固定Brief型を作らず、Task context、標準的な案内file、会話を利用する | 2026-09-05 |
| [Home Assistant: Automation basics](https://www.home-assistant.io/docs/automation/basics/) | 保存されたautomationがUI不在でもeventまたは時刻で実行されるmodel | 各回を新しいTaskにし、その時点のPermissionと状態を再評価してmissed runを自動補完しない | 2026-09-05 |

## Tool、Skill、拡張

| 参考 | 採用した考え方 | Eneでの差分 | 確認日 |
|---|---|---|---|
| [Model Context Protocol](https://modelcontextprotocol.io/specification) | Tool、Resource、PromptをProviderから独立したprotocolで接続する | Permission、Credential、Control planeはMCP contentから変更できず、Credentialはmodel contextへ露出しない | 2026-09-05 |
| [MCP Apps](https://modelcontextprotocol.io/extensions/apps/overview) | Toolが必要に応じて対話型UIを返す標準的な仕組み | 第一者UI全体の恒久置換には使わず、Tool単位のUIに限定する | 2026-09-05 |
| [Agent Skills specification](https://agentskills.io/specification) | 読み取り可能な手順、知識、付属resourceをportableなSkillとして配布する | Skillであること自体に成功検証を必須とせず、Ene側で由来、revision、実行結果を関連付ける | 2026-09-05 |
| [Agent Skills client implementation](https://agentskills.io/client-implementation) | 必要なSkillを段階的に読み込む実装model | 読込方式やcache方式は要件に固定せず、ユーザーから見える互換性だけを採用する | 2026-09-05 |
| [Hermes Agent: Skills System](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/features/skills.md) | Skillを必要時に読み込む再利用可能なknowledge/instructionとして扱い、agent自身も作成・更新できる | Experienceから再利用価値のある手順をSkill化し、import原本と学習revisionを区別する | 2026-09-05 |
| [nanobot: Skills](https://github.com/HKUDS/nanobot/blob/main/nanobot/skills/README.md) | `SKILL.md`を中心とする軽量なinstruction packageとしてSkillを扱う | Agent Skillsとの相互運用を優先し、Ene独自形式だけへ閉じない | 2026-09-05 |
| [LM Studio: Tool use](https://lmstudio.ai/docs/developer/openai-compat/tools) | Local model serverを既知のAPI互換面で直接利用する | 通常のProvider差をPlugin化せず、利用Capabilityとdata送信先をOwnerが割り当てる | 2026-09-05 |

Hermes AgentではMemoryを主にfacts、Skillsをproceduresとして分離し、nanobotでもSkillを具体的な手順やinstructionの単位として扱う。Eneもこの分離を参考にするが、MemoryとSkillに関連情報が存在すること自体を禁止せず、役割の違いと不要な重複回避を優先する。

## ProviderとPrompt cache

| 参考 | 採用した考え方 | Eneでの差分 | 確認日 |
|---|---|---|---|
| [OpenAI Prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching) | Cache利用量をProviderのusage情報として観測し、費用とlatencyを最適化する | Cacheを正本にせず、hitまたはmissで論理的context、安全境界、永続化契約を変えない | 2026-09-05 |
| [Anthropic Prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching) | Providerごとに異なるcache制御と課金情報があること | Provider固有のbreakpointやTTLを製品要件に固定しない | 2026-09-05 |
| [Gemini Context caching](https://ai.google.dev/gemini-api/docs/caching) | 明示cacheと暗黙cacheを含むProvider差を想定する | Cacheの有無をMemory、履歴、Task継続の契約にしない | 2026-09-05 |

最初のmilestoneでOpenAI Responses APIを使うことは[受け入れ条件](acceptance.md)のGateであり、恒久的な既定Providerという意味ではない。Promptの順序、固定prefix、cache key、TTL、context圧縮はProviderごとの設計と測定で決める。

## Permissionと安全

| 参考 | 採用した考え方 | Eneでの差分 | 確認日 |
|---|---|---|---|
| [MCP Security best practices](https://modelcontextprotocol.io/specification/draft/basic/security_best_practices) | 外部server、token、confused deputy、権限境界を明示的に扱う | MCP以外のLLM、Skill、Character、Workspace fileにも同じ信頼境界を適用する | 2026-09-05 |
| [OWASP: Prompt Injection](https://genai.owasp.org/llmrisk/llm01-prompt-injection/) | 外部content内の指示とOwnerの指示を分離する | Prompt防御だけに依存せず、機械的Capability境界とOwner確認を併用する | 2026-09-05 |
| [Android app-specific storage](https://developer.android.com/training/data-storage/app-specific) | App内部dataとOwnerが扱う通常fileのlifecycleを分ける | Host OSはDesktopだが、Ene内部dataと外部Workspace fileを分離する原則を採用する | 2026-09-05 |
| [Android shared documents](https://developer.android.com/training/data-storage/shared/documents-files) | ユーザーが選んだfileやfolderへの範囲限定access | DesktopでもOwnerがTaskへ接続した範囲と操作種別をPermission境界にする | 2026-09-05 |

## Setup、Backup、Reset、Remote

| 参考 | 採用した考え方 | Eneでの差分 | 確認日 |
|---|---|---|---|
| [Home Assistant onboarding](https://www.home-assistant.io/getting-started/onboarding/) | Local-first製品を少ない手順で利用可能にし、後から管理を拡張する | Companion、Cloud data egress、費用、model選択をEneのSetupへ加える | 2026-09-05 |
| [Home Assistant backup](https://www.home-assistant.io/common-tasks/general/) | Ownerが保存先、schedule、保持、保護、restoreを管理するfull backup | Eneはportable backupを提供し、Credentialと外部Workspace fileを除外する。暗号化は利用可能にするが全backupへの絶対要件にはしない | 2026-09-05 |
| [Chrome: Reset settings](https://support.google.com/chrome/answer/3296214?hl=ja) | 設定Resetとuser data削除を別操作にする | EneではLearning、Permission Rule、Provider同意、費用capも設定Resetから保護する | 2026-09-05 |
| [Blender: Factory settings](https://docs.blender.org/manual/en/latest/getting_started/configuration/defaults.html) | 初期設定への復帰を、作成dataの削除と分離して扱う | 全データResetは外部fileとOwner保存backupを対象外にする | 2026-09-05 |
| [Tailscale: What is Tailscale](https://tailscale.com/kb/1151/what-is-tailscale) | Owner管理VPNを通じたdevice間の保護された接続 | 特定VPNを必須にせず、Ene運営relayやaccountを置かない | 2026-09-05 |

## Accessibilityと品質

| 参考 | 採用した考え方 | Eneでの差分 | 確認日 |
|---|---|---|---|
| [WCAG 2.2](https://www.w3.org/TR/WCAG22/) | 重要情報のText代替とkeyboard操作を基本にする | 現段階ではVoice内容のText代替と、Mute、Stop、承認拒否のkeyboard経路を最低範囲とする | 2026-09-05 |

対象年齢、配布地域、地域法に基づくAI表示、Marketplaceの要否は、この確認時点で固定していない。公開判断の時点で、公式な法令、platform policy、各参考製品の最新状態を改めて調査する。
