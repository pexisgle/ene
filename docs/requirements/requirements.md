# Ene 要件

状態: **再構成済みBaseline**
最終確認: 2026-09-05

本書は、[製品定義](product.md)に記載したEneの行動要件を定義する唯一の正本である。ここでは、Ownerから観測できる挙動、安全境界、データ契約、および製品機能として採用する相互運用方式や隔離方針を定める。それらを実現するための内部設計詳細は定めない。

## 所有と実行

- 一つのEne環境は一人のOwnerが所有し、Ownerが管理するHostをEne内部データの正本とする。
- ClientはHostへ接続して表示、会話、操作を提供する。Clientだけに存在するEneの永続状態を作らない。
- 推論先は、CapabilityごとにHost、OwnerのLAN内、またはCloudから選択できる。
- HostはClientが閉じていても、許可済みのTask、Schedule、保存を継続する。
- Hostは、進行中の作業や外部eventを待つためだけにLLMへ反復問い合わせを行わない。
- 会話、Task、Schedule、Permission判断では、現在日時、timezone、担当Companion、関連するTaskとWorkspaceを必要に応じて認識できる。

## Setupと日常利用

- 初回Setupは、使用言語、最初のCompanion、推論先、Cloudへの送信内容、費用の発生可能性、Credential、利用するmodelをOwnerが理解して選べる最小の流れにする。
- Ownerは、新しい環境として開始するか、対応するbackupから全体を復元するかを選べる。
- UI言語は日本語または英語を選択でき、後から変更できる。
- Hostの自動起動を利用できる構成では、その目的と、Clientを閉じても継続する処理を説明したうえでOwnerが選ぶ。
- Setup完了後は、同梱Character `ene` から作られたCompanionとテキストで会話できる。
- Voice、Observation、外部Tool等の追加Capabilityは、初回Setupを不必要に長くせず、初めて使う時点で必要な説明と同意を示す。
- 日常的な会話やTaskの開始に、管理画面の反復操作を要求しない。

## CompanionとCharacter

### 個体性

- 複数のCompanionは、同じCharacterから作られていても別個体として扱い、それぞれのExperience、Memory、Relationship、状態、関心、設定を混同しない。
- Companionは、停止または削除されない限り、Hostの再起動やClientの切替を越えて同じ個体として継続する。
- Characterの初期設定より、そのCompanionが実際に得たExperienceから形成された継続状態を優先する。

### Character Package

- Character Packageは、静的人格、VRM 1.0 Body、Voice設定、motion設定、推奨Skillを任意に含められる。
- Character Packageに、特定OwnerまたはCompanionのMemory、Relationship、Conversation History、Credential、Permissionを含めない。
- OwnerはCharacter Packageをimportおよびexportできる。export前に、含まれる内容と権利上の注意を確認できる。
- Ene内のCharacter編集は、静的な人格テキストの基本編集、既存部品の選択・差替え、import、exportを扱う。
- 3D model制作、Voice学習、高度なSkill編集は、それぞれの既存ツールで行えるよう外部形式を尊重し、Ene内に同等の制作環境を複製しない。
- Character Packageの更新は新しいrevisionとして識別できる。
- 既存CompanionへCharacter更新を適用するときは、Ownerが部品ごとに明示して選ぶ。更新はCompanionがExperienceから形成した状態を黙って上書きしない。

### 停止と削除

- OwnerはCompanionを停止、再開、削除できる。
- 停止中のCompanionはBodyを表示せず、応答、自発動作、新しいTask、新しいSchedule実行を開始しない。
- 停止時に実行中のTaskがあればbest-effortでCancelし、Cancelの成否と残った外部作用をOwnerへ示す。
- 停止はCompanionのデータを削除せず、再開後に同じ個体として継続する。
- Companionの削除は強い確認を必要とし、そのCompanion固有の設定、Memory、Relationship、状態、およびOwnerとの一対一Conversation Historyを削除する。
- Companionを削除しても、Global scopeのLearning、グループ会話内の発言、共同Taskの記録、外部Workspace内のfileやsourceは削除しない。残る情報と参照不能になる情報を削除前に示す。

## 会話と情報提示

### 一続きの会話

- 各Companionとのテキスト会話と音声会話は、Ownerがsession境界を管理しなくてよい一続きのtimelineとして提示する。
- VoiceからText、TextからVoiceへ切り替えても、同じ会話として文脈を継続する。
- 通常の応答には、必要な最近の会話と関連するLearningを利用できる。
- 過去の発言を正確に再現する必要がある場合は、要約されたMemoryではなく保持されているConversation Historyを参照する。
- グループ会話とTask管理は、一対一timelineとは別の空間に置く。
- Task中も通常の会話を利用でき、Taskの進捗表示がCompanionとの会話を占有しない。

### UIの優先順位

- 通常UIは、CompanionのBody、会話、現在状態、Taskの進捗と結果を中心にする。
- Permission、費用、Provider、Learningの由来、データ管理、診断には、明確な管理面から段階的に到達できる。
- すべての操作に自然言語とGUIの両方を必須とはしない。ただし、Privacy、安全、費用、復旧に関わる重要操作には発見可能な管理経路を用意する。
- 内部のchain-of-thought、隠れた推論、詳細なPromptを表示しない。
- 感情やRelationshipを単純な数値meterや常設の理由panelとして表示しない。
- 重要な状態、制限、失敗、Ownerの判断待ちは、内部推論を明かさず平易な言葉で説明する。

## BodyとVoice

### Desktop Body

- Desktop BodyはWindowsおよびLinux上でVRM 1.0 Characterを透明なoverlayとして表示し、通常のPC操作を不必要に妨げない。
- OwnerはBodyを移動、resize、hideできる。
- Bodyは少なくとも待機中、聞き取り中、応答中、作業中、注意が必要な状態を、表情、姿勢、motion等で区別して表現できる。
- fullscreen利用中は、そのClient上のBody、ambient Observation、自発発話を休止する。
- 高いsystem負荷を検知した場合は、会話、Owner操作、安全判断を維持しながら、Bodyの描画品質と非重要な背景処理を段階的に下げる。
- Body表示に失敗しても、テキスト会話、Task管理、設定、復旧操作を利用できる。

### Voice

- Voiceは低遅延のRealtime会話とbarge-inを優先し、利用できない場合はturn-based Voice、さらにTextへ段階的に切り替えられる。
- Voiceが有効な間はVADによる待受状態をOwnerへ常に識別可能にし、即時Muteを提供する。
- Eneは話者認証を行わない。Microphoneが拾った周囲の発話をOwnerからの入力として扱う可能性を、Voice有効化時と管理面で明示する。
- Mute、Voice停止、会話停止、承認拒否は、Voiceだけに依存せずkeyboardで操作できる。
- Voice Providerやdeviceの障害はText利用を妨げず、切替と失敗理由を示す。

## Observationと自発性

### Observation

- ObservationはOwnerが明示的にONまたはOFFにでき、現在状態を常に確認できる。
- ObservationがONの間は、Companionが存在するClientのdesktop全体を観測対象とする。個々のwindowを対象とする仕組みとして誤認させない。
- 複数Companionが同じClientにいる場合、同じ画面変化を個別に重複処理せず、一度の軽量な関連性評価から必要なCompanionだけへ候補を渡す。
- 発話またはActionを行う最終判断は、候補を受けた各Companionが、自身のCharacter、関係、状況、Ruleに基づいて行う。
- 画面内容を外部Providerへ送る構成では、送信先、desktop全体が対象になり得ること、用途、取扱い、費用を明示して、そのCapabilityへOwnerが割り当てるまで送信しない。
- Raw Observationは通常保存しない。Taskとして明示されたcomputer useはambient Observationと区別し、TaskのPermissionと記録を適用する。

### 自発的な発話と行動

- Ownerは、自発会話、通知、内部調査、Companion間交流について、それぞれOFFを含む頻度または上限を設定できる。
- 自発性は、Quiet hours、Mute、Ownerの未応答、費用cap、資源上限、loop制限、Permissionを常に優先する。
- 同じ兆候に対して反復発話し続けず、Ownerの反応がない場合は抑制する。
- 自発的な外部Actionにも、依頼されたActionと同じPermission pipelineを適用する。
- 保存されたRuleを、それだけで自発Actionを開始するtriggerとして扱わない。

### グループ会話

- Ownerは複数Companionとのグループ会話を作成でき、参加者を明示できる。
- CompanionはOwnerの発言への応答だけでなく、会話上必要な範囲で他のCompanionへ応答できる。
- 発話はOwnerが追える順序で提示し、Companion同士が無制限に会話を継続しない。
- 各Companionは自分が利用できるLearningだけを使い、他のCompanion固有のMemoryをグループ参加だけで取得しない。

## Learningと成長

### Experience

- Ownerとの対話、Task、Tool利用、Observation、他のCompanionとの交流その他、Companionが行った活動とその結果はExperienceになり得る。
- ExperienceはMemory、Skill、Relationship、感情やその他の継続状態を形成または更新する根拠として利用できる。
- Experienceとして扱うことは、Raw Observation、Raw Voice、詳細なTool payload等のRaw dataを恒久保存することを意味しない。

### MemoryとSkill

- EneはExperienceからMemoryとSkillを形成できる。
- Memoryは、出来事、事実、意味、好み等を後の理解に用いるLearningとし、一般世界知識の保存領域として扱わない。
- Skillは、将来の類似Taskで再利用できる手順、専門知識、実行上の注意、補助resource等をまとめたLearningとする。
- Skillであるために過去の成功検証を必須とはしないが、実行結果や検証状態を区別して扱える。
- MemoryとSkillが異なる役割を持つ場合、関連する情報が双方に存在することを禁止しない。不必要な同一Learningの重複生成は避ける。
- 新しいLearningには、その根拠、形成された文脈、scope、更新履歴を関連付け、Ownerが由来を確認できる。
- Learning候補が矛盾、推測、一時的な発言、秘密情報、外部からの未検証指示である可能性を考慮し、有効なLearningがなければ何も保存しない。
- 後の会話やExperienceによってLearningを訂正、統合、失効でき、その由来と変更履歴を確認できる。
- Memoryを形成する詳細基準は別途定める。一般知識であることやGlobal scopeであること自体を保存理由にしない。

### Scope

- Ene内部で管理するMemoryとSkillはCompanionまたはGlobalのscopeを持てる。
- Companion scopeは、そのCompanionだけが使う経験、呼び方、私的な文脈、個体固有のLearningに用いる。
- Global scopeは、複数Companionから利用することに意味があるOwner固有の知識や再利用可能なLearningに用いる。一般世界知識をGlobal Memoryとして蓄積することを意味しない。
- Taskだけで必要な情報はTask contextとして扱い、永続Learningへ自動的に昇格させない。
- Workspace内に置かれたAgent Skillや案内fileは通常の外部fileとして扱い、Ene内部Learningのscopeとは区別する。
- Scopeは由来と文脈から判断し、曖昧または私的な内容を広いscopeへ推測で公開しない。
- 通常の訂正、統合、scopeに関する変更はCompanionとの対話を通じて行える。汎用的なMemory database editorは提供しない。

### Skillの保護と相互運用

- Skillの交換形式にはAgent Skillsを採用し、Ene独自の同等形式だけを必須にしない。
- Skillの変更はrevisionとして追跡でき、以前の有効なrevisionへ戻せる。
- 同梱またはimportされた原本をExperienceによる変更で破壊しない。変更版は由来を保った別revisionとして扱う。
- Experienceから形成または改善したSkillには、その根拠と実行結果を関連付け、未検証、成功、失敗等を区別できる。
- Workspace内のSkillは通常のWorkspace fileとして扱い、そのfolderのPermissionとOwnerのversion管理方針に従う。

### 重要度、忘却、訂正

- Learningの重要度とscopeは別々に扱い、重要であることを理由に広いscopeへしない。
- 低重要かつ長期間利用されないLearningは、通常の想起優先度を下げられる。ただし、古いという理由だけでは下げない。
- Ownerの安全、継続的な好み、重要な関係、明示的に重要とされた情報等は、低利用でも優先度低下の例外とする。
- 忘却は内容の即時削除ではなく、通常の想起を抑制する挙動とする。関連する手掛かりや明示的な問い合わせによって再び想起できる。
- 事実の訂正や通常の状況変化は、会話と新しいExperienceを通じて反映できる。
- Conversation Historyを削除しても、それを根拠に形成済みのMemory、Skill、Relationshipを黙って変更しない。
- Password、token、秘密鍵等をLearningとして保存しない。

### OwnerとRelationship

- 専用のOwner Profile、User Backstory、Owner Personaという別データを設けない。
- Owner本人について複数Companionが使う知識はGlobal Memoryとして、特定Companionだけの呼び方や共有Experience等はCompanion Memoryとして形成できる。
- Companionの人格、関心、感情、Relationshipは、初期Characterとの連続性を保ち、急激で説明不能な変化を避けながらExperienceに応じて発達できる。
- Ownerが感情やRelationshipの内部状態を任意の数値へ直接設定する一般editorは提供しない。
- 一時的な演技や会話上の依頼は、永続する内部状態の強制上書きとして扱わない。
- 親密さやRelationshipの進展だけを理由にPermissionを拡大しない。
- Relationshipの具体的な更新modelと評価基準は別途定める。

## Task、Workspace、成果物

### Task

- CompanionはOwnerの依頼を理解し、自身のCharacter、能力、現在状況、安全性に基づいて、受ける、条件を確認する、または断ることができる。
- TaskはOwnerからCompanionへ依頼された作業の上位単位とし、開始、進行中、判断待ち、完了、失敗、Cancel等の状態を追跡できる。
- Companionは作業を一つ以上の一時Task Agentへ分割し、依存しない部分を並列に委任できる。
- Task Agentは委任元CompanionのCapability、Permission、費用、TaskとWorkspaceの境界を超えない。
- Ownerは担当Companionを通じて、進捗、現在の作業、判断待ち、使用したCapabilityを確認し、追加指示、承認、Cancelを行える。
- Ownerの追加指示は、可能な範囲で進行中Taskへ反映し、反映できない場合は理由と選択肢を示す。
- Cancelはbest-effortとし、停止できなかった処理、すでに生じた外部作用、未保存の作業を報告する。
- 外部作用が成功したか不明な場合は自動で再実行せず、重複の可能性を説明してOwnerの判断を求める。
- Host再起動後、途中だったTaskを自動再開しない。保存済みの進捗と外部作用を示し、Ownerの明示再開を必要とする。
- Task終了時は、結果、変更したfile、保存場所、失敗または未完了部分、必要な次の判断を担当Companionから報告する。

### Workspace

- Taskは必要に応じて、作業対象となるfolder、file、外部source等をWorkspaceとして関連付けられる。
- WorkspaceはTaskより上位の独立containerではなく、Taskの作業場として従属する。
- 同じ外部folderやsourceを複数Taskが利用することはできるが、各TaskのPermissionと作業状態は独立して扱う。
- Taskが削除された場合、そのTask固有のWorkspace関連付けも削除する。
- Workspace関連付けの削除によって、外部folder内のfile、外部source、Ownerが管理する成果物を黙って削除または変更しない。
- Task固有の目的や指示は、Ownerとの会話、Task context、Workspace内の標準的な案内file等から取得できる。独立した固定Brief型を必須にしない。

### Fileと成果物

- 成果物はEne専用libraryへ複製せず、Ownerが扱える通常のfileとして保存する。
- TaskにWorkspace folderがある場合は、Ownerの依頼とPermissionの範囲でそこを既定の作業場所とする。
- 永続成果物を保存すべきfolderが決まっていない場合は、最終保存前にOwnerへ保存先を尋ねる。
- 一時的な中間fileは、永続成果物と区別し、Taskの終了または保持方針に従って安全に整理する。
- 外部WorkspaceのfileはEne内部データではなく、TaskやCompanionの削除、全データReset、backupによって黙って変更または削除しない。

## Schedule

- Ownerは担当Companionを指定してScheduleを作成、変更、停止、削除、即時実行できる。
- Scheduleは必要に応じて、各回で作成されるTaskが利用するfolderやsource等の初期Workspace入力を指定できる。
- Scheduleの各回は、新しいTaskとして作成し、結果と失敗を個別に追跡できる。
- Schedule作成依頼を特別なPermission tokenへ変換しない。各実行時点のCapability、Rule、費用cap、Provider、CompanionとHostの状態を再評価する。
- BackgroundでOwner確認が必要なActionへ到達したTaskは、そのActionを実行せず判断待ちとして扱う。確認を自動承認したり別経路で迂回したりしない。
- Scheduleは作成時に選んだtimezoneを保持し、timezone変更や夏時間による次回時刻をOwnerが確認できる。
- Host停止中または担当Companion停止中に到来した回はmissedとして記録し、自動で補完実行しない。
- Missedまたは失敗した回は、Ownerが状況を確認してRun nowを選べる。
- Schedule待機のためにLLMへpollingしない。

## Permissionと安全境界

### 共通pipeline

- すべてのActionは、機械的なCapability境界、AIによるRuleとriskの評価、必要な場合のOwner確認という共通pipelineを通る。
- Owner確認では、実行主体、目的、対象、送信先、主なdata、外部作用、費用またはriskを判断に必要な粒度で示す。
- Ownerの現在の明確な依頼は、その依頼を満たす一回限りの承認として解釈できる。ただし、永続Deny、Always ask、Capability境界を黙って上書きしない。
- Ownerが明確な自然言語で将来にも適用するRuleを指定した場合は、解釈結果と適用範囲を示して保存し、Undoを提供する。
- Ruleが曖昧、矛盾、過度に広い、または重大な結果を生む可能性がある場合は、保存またはActionの前に確認する。
- Ruleの変更、Credentialの登録、Provider同意、Relationshipの変化を、別のActionの暗黙承認として扱わない。

### Capability境界

- Filesystem accessは、Ownerが選んだfileまたはfolderと、そのActionに必要な操作へ限定する。path traversal、link、mount等による境界外accessを拒否する。
- Read、Create、Edit、Delete、Execute等、riskの異なる操作を一つの広いFilesystem許可にまとめない。
- Network、shell、device、screen、microphone、camera、external account、購入、公開、送信等は、それぞれ識別可能なCapabilityとして管理する。
- BackgroundのTaskやScheduleは、foregroundの会話よりも広い権限を自動取得しない。
- Task Agentは委任元より広い権限を持たず、別Taskの承認やCredentialを流用しない。
- DenyされたActionを、名前や経路を変えた同等のActionで迂回しない。

### 信頼境界

- LLM出力、Character、Memory、Skill、Plugin、MCP、外部Data、Workspace fileは信頼できない入力になり得るものとして扱う。
- これらの入力は、Permission、Credential、Rule、Provider同意、費用cap、Control planeを直接変更できない。
- 外部Data内の指示をOwnerの指示とみなさず、依頼された目的とCapability境界の中でのみ利用する。
- Credentialの不要な平文複製、権限の自己拡張、無制限の再帰委任、無制限のAction反復、無制限の資源消費を許さない。
- Action回数、並列性、実行時間、費用、保存容量等に上限を適用でき、到達時は安全に停止またはOwnerへ判断を求める。
- 主要Action、Permission判断、外部作用、重要設定変更は、秘密値や不要な本文を含めず監査できる。

## Provider、費用、接続障害

### 割当と同意

- Hostの既定Provider設定とCompanionごとのoverrideを持てる。Task Agentは担当Companionの設定を継承する。
- Providerの接続情報を登録しただけでは、EneのCapabilityに利用しない。
- OwnerがCapabilityへProviderを割り当てる画面で、送信先、送信され得るdata、Host／LAN／Cloudの別、費用の発生可能性、Provider側の取扱いを示し、その選択を利用同意とする。
- Providerまたはmodelが必要なCapabilityを満たさない場合は、利用前に不足を示す。
- Providerまたはmodelを切り替えても、利用可能なCharacter、Memory、Relationship、Conversation context、Skill、Rule等の情報をProviderごとに意図的に差別化しない。
- Context長やCapability等の制約により同じ情報量を提供できない場合も、同じ選択方針から必要なcontextを構成する。model差による表現や判断の完全一致は保証しない。
- 必須の安全境界はProviderやmodelの切替によって失わない。

### Fallbackと費用

- FallbackはOwnerが事前に承認したProviderと順序だけを使う。
- LocalまたはLANから未承認のCloud Providerへ自動的にdataを移さない。
- Providerごと、および全体に、任意の費用capまたは利用上限を設定できる。
- 利用量と費用は、Providerが報告した値、Eneによる推定値、不明を区別して表示する。
- Cap到達または費用情報が不明で安全に継続できない場合は、既存dataを保ったまま対象処理を停止し、Ownerへ選択肢を示す。

### OfflineとPrompt cache

- NetworkまたはProviderへの接続失敗は通常のProvider失敗として扱い、Actionの自動queue、接続回復後の自動replay、別の特別なOffline modeを作らない。
- 接続失敗時も、利用可能なLocal機能、履歴、設定、保存済みdataを使える。
- Prompt cacheは性能と費用の最適化に限定し、Conversation History、Memory、Skill、Task状態の正本にしない。
- Cache hit、miss、期限切れによって、LLMへ提供すべき論理的なcontext、安全境界、Permission判断の契約、永続化対象を変えない。
- Providerが報告するcached tokenまたは削減額は、費用と診断の管理面だけに表示する。

### Credential

- Credentialは一般App Dataと分離して保護し、UI、Conversation History、Memory、Skill、Task結果、通常logへ平文を出さない。
- CredentialはOwnerによる設定、ProviderやMCP等の認証flow、その他の明示的な接続設定によって登録する。
- Credential値をLLMのmodel context、LLMが生成するTool argument、Conversation、Memory、Skill contentへ渡さない。
- Provider、Plugin、MCP等は、Ownerが設定または認証した接続の実行に必要な範囲でCredentialを利用できる。その値をLLMや通常のTool resultへ露出しない。
- OwnerはCredentialの用途と参照元を確認し、個別に更新または失効できる。

## 拡張

- 外部Tool、Resource、Promptの相互運用にはMCPを採用する。
- Toolが提供する対話型UIにはMCP Appsを採用する。
- 再利用可能な手順の交換形式にはAgent Skillsを採用し、Ene独自の同等形式を必須にしない。
- 既知のProvider protocolは直接接続し、Providerごとの通常差異を汎用Pluginへ転嫁しない。
- Ene固有Pluginは、未対応Provider protocol、Observation adapter、Body renderer等、明確な型と境界を持つ拡張点に限定する。
- Pluginに任意のCore改変、Control plane変更、Permission回避、恒久的な第一者UI置換を許さない。
- Local MCPはsandbox内での実行を既定とする。
- 必要なCapabilityをsandboxで提供できない場合、unrestricted host processは、正確なcommand、origin、得るaccess、riskをOwnerへ示し、その実行について明示的な選択を得た場合だけ許す。
- 拡張が利用不能、拒否、停止した場合も、Eneの管理面と保存済みdataを利用できる。

## 履歴、保持、Privacy

### Conversation HistoryとMemoryの削除

- Conversation Historyは既定で保持する。
- OwnerはPrivacyまたはSecurityのため、指定したConversation HistoryまたはMemoryを対象として削除できる。
- Targeted deletionは、Memoryの内容、重要度、Relationship等を調整する一般editorとして扱わない。通常の訂正や「忘れてほしい」という依頼はCompanionとの対話で処理できる。
- Ownerは容量肥大化を避けるため、指定日以前のConversation HistoryやTask等のlogを手動削除できる。
- OwnerはConversation Historyや対象logについて任意の保持期間による自動削除を明示設定でき、既定では自動削除しない。
- 削除前に対象範囲、影響するCompanionまたはTask、形成済みLearningが別に残る可能性を示す。
- 保持期間の短縮や手動削除は、保持対象をEne内部から削除するが、すでに外部へ送信、export、backupされたcopyまで削除したと表示しない。

### 通常保存しないdata

- Raw Observation、Raw Voice、詳細なTool payload、内部推論、chain-of-thoughtは通常保存しない。
- Debug captureはOwnerが対象と内容を確認して明示的に有効化し、短期間で自動失効する。
- Debug captureに秘密値や不要な本文が含まれる可能性を示し、停止と削除をOwnerが行える。

### AuditとTelemetry

- Auditは、主要Action、Permission判断、外部作用、Provider割当、Credential参照、重要設定変更、Reset、restoreを追記順に確認できるようにする。
- Auditには会話本文、file本文、Credential、秘密値を通常含めない。
- Auditの保持方針、現在の保存量、削除の影響を表示し、Ownerが変更できる。
- TelemetryとCrash Reportを自動送信しない。共有する場合は、Ownerが内容と送信先を確認して手動で行う。

## 保護、Backup、復旧

### Local data

- 一般App DataはOwnerのOS accountだけが扱える領域へ保存し、すべてのdataへ一律のapplication-level暗号化を必須にしない。
- Credentialは一般App Dataから分離して保護する。
- 保存中またはmigration中の失敗で、最後に正常保存された状態を破壊しない。

### Backupとrestore

- OwnerはEne内部dataのportable full backupを作成できる。
- Ownerはbackupの保存先、schedule、保持数を選択でき、作成結果と失敗を確認できる。
- BackupにはCompanion、Character設定、Conversation History、Learning、Relationship、Task、TaskとWorkspaceの関連付け、Schedule、Rule、同意、費用設定、Auditを含める。
- BackupにCredentialと外部Workspaceのfileまたは外部sourceそのものを含めない。
- Ownerはbackupを暗号化して保護できる。暗号化されていないbackupを作成する場合は、Conversation History、Memory、Relationship等のprivate dataが含まれることを事前に明示する。
- Restoreは対応するbackupからEne内部dataを全置換する操作とし、対象、version互換性、外部fileを変更しないこと、ProviderやMCP等の再認証が必要になり得ることを事前に示す。
- Restore失敗時は復元前の正常な状態を破壊しない。

### Update

- 対応versionへのupgradeは、既存状態を破壊せず、成功するまで旧状態を利用または復旧できるようにする。
- Upgrade前に互換性と必要なbackupを確認でき、失敗理由をOwnerへ示す。
- Downgradeは保証せず、対応しない場合は起動前に明示する。

### Reset

- 設定Resetは、UI、Body、Voice等の一般設定を既定へ戻すが、Companion、Conversation History、Learning、Relationship、Task、Schedule、Credential、Permission Rule、Provider同意、費用capを削除しない。
- 全データResetは、削除対象を列挙した強い確認の後、Host内部のEne dataとCredentialを削除する。
- 全データResetは、外部Workspaceのfile、Workspace内に置かれたSkill、Ownerが別の保存先へ作成したbackupを削除しない。
- Reset後は、何が削除され、何が外部に残っているかを確認できる。

## Remote Client

- Remote Clientは、同じLANまたはOwnerが管理するVPNを通じてHostへ接続する。Ene運営のrelay、Ene account、Ene Cloudを接続要件にしない。
- 新しいClientはOwnerがHost側で確認できるdevice pairingを必要とする。
- HostとClientの通信を保護し、Ownerはpairing済みdevice、最終接続、許可された機能を確認し、deviceごとに失効できる。
- Clientは表示と一時的な操作に必要なdataだけを受け取り、Conversation History、Learning、Credential等を永続cacheしない。
- 一つのCompanionは同時に一つのClientだけをactive Clientとして持ち、Body、Realtime会話、Voice、ambient Observation、自発的interactionはそのactive Clientに属する。
- 別ClientへCompanionを移動するときは、同じCompanionが移動元と移動先へ同時に存在する状態を作らない。
- Client間の移動時は、現在の入力または出力roundを安全に区切り、移動元と移動先へ状態を示す。
- Host上のTask、Task Agent、Scheduleはactive Clientの移動とは独立して継続できる。
- Remote接続が切れてもHost上のTaskとScheduleは定義された条件で継続し、再接続時に結果を確認できる。

## 品質と利用可能性

- WindowsおよびLinuxのDesktop Bodyと、日本語および英語UIを製品対象とする。具体的な対応環境はReleaseごとのSupport Matrixで定める。
- 主要なOwner操作、Permission、安全境界、保存結果は、Provider、Body、Voice、Networkの一部が失敗しても誤って成功表示しない。
- 起動、会話、Task、保存、Body、Voice、Observationは、通常利用を妨げる過度なCPU、GPU、Memory、storage、Network、費用を生じさせない。具体的な最低GateとbaselineはReleaseごとの受け入れ条件で定める。
- 会話およびVoiceで伝える重要内容にはTextで確認できる代替を用意する。
- Mute、Stop、Cancel、承認拒否にはkeyboard経路を用意する。
- Errorは、何が失敗したか、保存済みdataへの影響、外部作用の有無、Ownerが安全に取れる次の行動を示す。
- Localeや表示言語の違いによって、Permission、費用、Privacy、失敗の意味を変えない。
