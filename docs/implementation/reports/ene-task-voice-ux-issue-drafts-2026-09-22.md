2026-09-22 / Task関連UXのIssue案。同日に起票済み。

親Issue: [#1686 音声対話をTask操作の主経路とし、ユーザー向け作業一覧とGUI報告を不要にする](https://github.com/pexisgle/ene/issues/1686)

| 順 | Issue | 内容 |
|---|---|---|
| 1 | [#1687](https://github.com/pexisgle/ene/issues/1687) | 不要な作業一覧・番号・GUI報告を削除する |
| 2 | [#1688](https://github.com/pexisgle/ene/issues/1688) | 音声によるTask操作と割り込みを通す |
| 3 | [#1689](https://github.com/pexisgle/ene/issues/1689) | 会話の文脈から操作対象を特定する |
| 4 | [#1690](https://github.com/pexisgle/ene/issues/1690) | 進捗・完了・確認待ちを自然な音声で伝える |
| 5 | [#1691](https://github.com/pexisgle/ene/issues/1691) | 依頼時の条件確認を対話につなぐ |
| 6 | [#1692](https://github.com/pexisgle/ene/issues/1692) | Web・MCP・PC操作へ実行能力を広げる |
| 7 | [#1693](https://github.com/pexisgle/ene/issues/1693) | 定期的な仕事を音声で設定・変更する |
| 8 | [#1694](https://github.com/pexisgle/ene/issues/1694) | 作業手順を会話からSkillとして再利用する |
| 9 | [#1695](https://github.com/pexisgle/ene/issues/1695) | 並行作業と引き継ぎをCompanionが調整する |
| 10 | [#1696](https://github.com/pexisgle/ene/issues/1696) | 別端末から同じCompanionの仕事を継続して扱う |

依存関係は [#1687](https://github.com/pexisgle/ene/issues/1687) が #1688・#1689・#1690・#1691 の後、[#1690](https://github.com/pexisgle/ene/issues/1690) が #1688・#1689 の後とした。以下は起票時の本文案であり、内容の正本はGitHub側のIssueとする。

**Companionとの音声対話だけで、仕事の依頼・途中変更・状況確認・中止・再開まで進められるようにする。** 「作業1」「作業2」という一覧、その選択、更新、GUIでのTask報告は、この体験から除く。Taskの識別・記録・安全な実行管理は内部で維持する。

この方針は、比較調査後のユーザー指示に基づく。前回の「作業名を分かりやすくする」「一覧・進捗表示・GUI通知を充実させる」という提案は撤回する。成果物の確認・プレビュー・保存操作の改善は今回の対象外とし、作業UIの削除に巻き込まない。

目指す利用例は「このフォルダを調べて」→必要な条件をCompanionが尋ねる→裏で作業が進む→作業中も雑談できる→「さっきの調査、要約だけでいいよ」で変更→完了や要対応事項をCompanionが音声で伝える、という流れである。ユーザーにTask番号やTask Agentの選択を要求しない。

通常のTask報告をGUIへ出す必要はない。音声障害時のテキスト代替、アクセシビリティ、設定・認証・緊急停止の経路は用途を区別して設計する。これらのために普段の作業一覧や報告画面を復活させない。

親Issue案: **[設計] 音声対話をTask操作の主経路とし、ユーザー向け作業一覧とGUI報告を不要にする**

現在の要件はタスク管理を別画面に分け、進捗を画面の主役とし、未伝達報告を実際の画面提示によって確認する。この記述を最新のユーザー方針に合わせる必要がある。VoiceがStage 10に置かれている実装順も、音声によるTask操作の最小経路をいつ完成させるかという観点で見直す。既存のStage完了記録は、その時点の検証範囲として扱い、新しい音声UXの完成を意味するものにしない。

完了条件は、製品要件・受け入れ条件・関連する上位設計で、通常のTask操作と報告に作業GUIを要求しないことが一致すること。音声による報告を「提示できた」と扱う条件、音声が使えない場合の代替、安全操作の権限境界を定義する。発話を生成したこと、送信したこと、再生が完了したこと、ユーザーが実際に聞いて理解したことを混同しない。

根拠: [画面中心の要件](/home/pexisgle/dev/Ene/docs/requirements/requirements.md:71)、[Voice要件](/home/pexisgle/dev/Ene/docs/requirements/requirements.md:92)、[画面提示に依存する報告確認](/home/pexisgle/dev/Ene/docs/requirements/requirements.md:202)、[入出力と未伝達提示の設計](/home/pexisgle/dev/Ene/docs/design/subsystems/client-presence-io-observation.md:55)、[実装順](/home/pexisgle/dev/Ene/docs/implementation/README.md:81)。設計の変更を先に確定し、下位実装だけで意味を変えない。

| 子Issue案 | 扱う問題 | 完成後の体験 |
|---|---|---|
| 1. [UX] 不要な作業一覧・番号・GUI報告を削除する | 内部Taskがユーザー向けの操作単位として露出している | 作業パネルを開かずに仕事を任せられる |
| 2. [Voice] 音声によるTask操作と割り込みを通す | 音声が後続Stageにあり、現在の依頼経路はテキスト中心 | 話しかけて依頼し、作業中も話せる |
| 3. [Task] 会話の文脈から操作対象を特定する | 操作対象が単一のcurrent TaskとGUI選択に依存する | 「さっきの調査を中止して」が対象を取り違えずに届く |
| 4. [Companion] 進捗・完了・確認待ちを自然な音声で伝える | 状況の取得、伝達、報告文が通常対話へ統合されていない | 必要なときに状況が分かり、聞き返せる |
| 5. [Workspace/Permission] 依頼時の条件確認を対話につなぐ | Workspace未選択時などにGUI操作を求めて会話が途切れる | 足りない情報を会話で補い、許可された範囲で続けられる |
| 6. [Execution] Web・MCP・PC操作へ実行能力を広げる | Task Agentの道具がフォルダ内の4操作に限られる | 複数のアプリや情報源を使う仕事を任せられる |
| 7. [Schedule] 定期的な仕事を音声で設定・変更する | Scheduleが未実装 | 「毎朝9時に」「明日からやめて」で管理できる |
| 8. [Learning] 作業手順を会話からSkillとして再利用する | 成功した手順を次回の仕事へつなげられない | 「次もこのやり方で」と頼める |
| 9. [Collaboration] 並行作業と引き継ぎをCompanionが調整する | ユーザーが実行担当や引き継ぎ先を仲介する体験との差 | 複数の仕事を頼んでも、Companionが調整して報告する |
| 10. [Remote] 別端末から同じCompanionの仕事を継続して扱う | remote Clientが後続計画 | 現在の端末から同じ仕事について話せる |

子Issue 1の本文案:

作業一覧を「良い名前の一覧」に置き換える改善は行わない。通常画面から番号付きTask一覧、Task選択による会話対象の変更、進捗表示、更新ボタン、Task報告の専用表示を除く。内部のTask、結果、実行事実、未伝達事項は削除しない。成果物確認の機能はこのIssueの変更範囲から外す。

完了条件は、通常のTask依頼から終了までに作業GUIを開く手順がなく、Task番号が表示・読み上げされないこと。既存の中止・再開・Workspace選択を単に失わせず、子Issue 2・3・5の対話経路と、必要な緊急操作が成立した状態で削除を完了する。作業パネルの代わりとなる別のTaskダッシュボードは追加しない。[現行の一覧生成](/home/pexisgle/dev/Ene/apps/ene-desktop/src/ui/tasks.rs:75)、[現行画面](/home/pexisgle/dev/Ene/apps/ene-desktop-ui/ui/chat.slint:1)

子Issue 2の本文案:

音声入力から、依頼の解釈、既存のTask制御、Companionの発話までの最小経路をつなぐ。音声による依頼を別のTask管理実装にせず、現在の会話・Taskの境界を利用する。Task Agentが動いている間もCompanionとの会話を続けられるようにする。

完了条件は、音声で作成・状況確認・変更・中止・再開ができること。発話への割り込み、読み上げの停止、仕事の中止を区別する。たとえば「説明はもういい」で音声を止めても、進行中の仕事を勝手にキャンセルしない。マイクや音声サービスの障害でも、既に受け付けた仕事を失ったり自動で再実行したりしない。音声認識から実行までを通す実機受け入れを含め、文字列を直接注入するテストだけで完了扱いにしない。[Voice要件](/home/pexisgle/dev/Ene/docs/requirements/requirements.md:92)、[現在の会話からTaskへの入口](/home/pexisgle/dev/Ene/crates/ene-companion/src/dialogue.rs:754)

子Issue 3の本文案:

現在はCompanionごとに一つのTaskへの一時的な対応を持ち、GUI選択でこれを切り替える。音声中心のUXでは、依頼内容、直前の会話、担当、保存済みの状況から操作対象を特定する必要がある。音声で番号を選ばせる仕組みへの置き換えは避ける。

完了条件は、複数の仕事があっても「さっきの調査」「フォルダ整理のほう」などで対象を指定できること。曖昧なときは短く聞き返し、無関係な仕事へ変更や中止を送らない。Host再起動後も保存された事実から候補を確かめられ、GUIでの再選択を求めない。対象を特定できたことと、その時点の状態で変更・再開できることは分けて検証する。内部検索は件数を制限し、先頭50件に含まれない仕事も対象にできる。[現在の対応管理](/home/pexisgle/dev/Ene/apps/ene-core/src/task_control.rs:143)、[内部コマンド](/home/pexisgle/dev/Ene/crates/ene-companion/src/dialogue.rs:1248)

子Issue 4の本文案:

Taskの状況をCompanionが把握し、問い合わせへの返答と、完了・中断・ユーザー判断が必要な変化の音声報告へつなぐ。固定英語の受付応答や`adopted`などの内部用語は、自然な日本語の説明へ置き換える。操作ログをそのまま読み上げず、進めた内容、確定した変更、残作業、必要な判断を短く伝える。成果物の存在や保存場所への言及は含むが、成果物確認UIは変更しない。

完了条件は、中断しているのに「作業中」と伝えず、キャンセル受付を実際の停止完了として報告しないこと。結果不明の作用を成功や失敗と断定しない。発話抑制や切断中に伝えられなかった事項を保存し、対話を再開したときに現在の状況へまとめ直せること。出力ミュート、静粛時間、割り込み、再生失敗、再接続を扱い、生成・送信だけで報告済みにしない。変更のない進捗を繰り返し話し続けない。音声の提示確認と作業の成功・操作への承認は別の事実として扱う。[現在の未伝達取得API](/home/pexisgle/dev/Ene/apps/ene-desktop/src/ui/runtime.rs:1040)、[現在の報告文](/home/pexisgle/dev/Ene/crates/ene-companion/src/dialogue.rs:1470)、[Task状態の意味](/home/pexisgle/dev/Ene/crates/ene-task/src/task.rs:140)

子Issue 5の本文案:

Workspaceや許可が足りないとき、Companionが不足する情報を会話で確認し、その依頼の続きとして扱う。すでに選択・許可されている範囲は利用し、「先にパネルで設定して、もう一度依頼して」と会話を途切れさせない。作業対象を音声で指定する経路と、実際にアクセスを許可する経路を設計する。

完了条件は、フォルダ名が曖昧なら聞き返し、確認できた対象について既存のWorkspace境界を維持できること。必要な確認を何度も最初からやり直さず、拒否・取消をその依頼に正しく反映する。音声の認識結果やモデルの出力だけで、第一者確認を済ませた扱いにはしない。現行設計は声による話者認証を行わないため、音声の了承をどの条件で正式な許可として受理できるかは先に設計で確定する。認証情報の入力など専用の安全な経路が必要な操作は、理由を会話で案内する。[Workspace未選択時の処理](/home/pexisgle/dev/Ene/apps/ene-core/src/task_control.rs:408)、[音声と話者認証の設計](/home/pexisgle/dev/Ene/docs/design/subsystems/client-presence-io-observation.md:56)、[第一者確認の境界](/home/pexisgle/dev/Ene/docs/design/concrete/first-party-desktop.md:140)

子Issue 6〜10は、機能拡張の追跡として扱う。既存ロードマップを使い、未着手機能を現在の実装の不具合と混同しない。

| 案 | 本文へ入れる範囲・受け入れ条件 | 既存計画との関係 |
|---|---|---|
| 6 | Webからの情報取得、MCPツール、PC操作をそれぞれ小さく分ける。許可された実ツールを通した仕事が完了し、利用不能・結果不明・人の介入待ちを音声で伝える。PC操作中はユーザー入力で中断できる。 | MCP/Computer Useの要件あり。現行runnerはlist/read/create/edit。対象別に実装計画を立てる |
| 7 | 会話で担当・時刻・タイムゾーン・内容を設定し、変更・停止・手動実行できる。次回予定や直近の実行結果を尋ねられる。Host停止中の回は要件どおりスキップし、無断でまとめて実行しない。 | Stage 8 |
| 8 | 完了した仕事の手順を会話からSkillとして保存・修正・再利用できる。スキルの採用と実行権限を分け、未検証の手順を実績扱いにしない。 | Stage 13 |
| 9 | 複数の仕事の実行と結果統合をCompanionが調整する。必要な担当変更やCompanion間の引き継ぎを説明でき、ユーザーがTask Agentを手動で振り分けなくてよい。記憶の共有範囲は維持する。Grok Botのグループ画面の複製は要求しない。 | Taskの委任基盤、Stage 12。子Issue 3は対象特定、このIssueは実行側の協調を扱う |
| 10 | 別端末で同じCompanionへ仕事の状況を尋ね、変更・中止できる。帰属する端末を切り替え、古い音声入力や操作を新端末で再送しない。 | Stage 14。Hostが停止した状態でのクラウド継続は追加しない |

根拠: [Task Agentの道具](/home/pexisgle/dev/Ene/crates/ene-task/src/agent.rs:637)、[Computer Use要件](/home/pexisgle/dev/Ene/docs/requirements/requirements.md:208)、[Schedule要件](/home/pexisgle/dev/Ene/docs/requirements/requirements.md:226)、[MCP要件](/home/pexisgle/dev/Ene/docs/requirements/requirements.md:278)、[既存ロードマップ](/home/pexisgle/dev/Ene/docs/implementation/README.md:81)。

起票と実装は、親Issueで要件・設計・音声の最小受け入れを確定した後、子Issue 2〜5で操作・報告の経路を通し、子Issue 1の削除を完了させる順が妥当である。子Issue 1の着手は並行できるが、操作経路を失った状態を完成としない。子Issue 6〜10はロードマップ上の拡張として追跡する。

前回挙げた「中断状態をカードで表示する」「一覧を自動更新する」「ページ送りを足す」「GUI通知を出す」は独立した改善Issueにしない。必要な状態把握・履歴取得・未伝達管理を子Issue 3・4へ引き継ぐ。成果物確認の差分は今回起票しない。手元Hostでの実行、Companionと一時的なTask Agentの分離、結果不明時の自動再実行禁止は維持する。

公開中のIssueを確認したところ、この音声中心のTask UXを直接扱うIssueは見当たらなかった。起票後も要件・設計・実装コードは変更していない。前回の調査で確認したコード上の事実を再利用しており、今回はテストを再実行していない。
