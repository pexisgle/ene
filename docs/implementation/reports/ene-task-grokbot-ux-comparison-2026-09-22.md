調査日: 2026-09-22。対象: `663b2075` を基準とする調査時の作業ツリー。

追記: 調査後のユーザー指示により、音声対話を前提とし、番号付き作業一覧とGUIでのTask報告は不要とする。本書の作業名・一覧・GUI通知を充実させる改善提案は撤回し、対応は[#1686](https://github.com/pexisgle/ene/issues/1686)とその子Issueで追跡する。成果物の確認は今回の修正対象外。本書の実装調査結果は調査時点の記録として残す。

**eneには「会話で実作業を頼み、裏で実行させ、途中で指示を変える」仕組みがあります。ただし、現在のtask関連UXをGrok Botと同等とは評価できません。** 実行できる仕事の範囲と、進捗・中断・成果物・完了報告を画面で扱う体験に差があります。優先すべきは、既に保存しているタスクの事実を、ユーザーが判断できる表示へつなぐことです。

この文書は比較調査であり、新しい製品要件やStageの完了判定を定めません。開始時から未コミットだったdesktopの4ファイルも現在の実装として読みましたが、変更していません。Grok Botは公式サイトと公式ドキュメントを参照しました。ログインした実機での操作、応答速度、自然言語の理解精度は今回の検証対象外です。段階提供の機能は、全アカウントで使えるとは扱いません。

比較する概念をそろえる必要があります。Grok BotのBotは、名前・役割・会話・記憶を持つ継続的な担当者です。eneではCompanionがこれに近く、Taskは追跡する仕事、Task Agentは仕事を任される一時的な実行者です。Task Agentに人格や長期的な関係性を持たせないことは、eneの明示的な製品方針です。[Grok BotのBot定義](https://docs.x.ai/grok-bot/bots)、[eneの製品定義](/home/pexisgle/dev/Ene/docs/requirements/product.md:47)

Grok Botの基本的な操作は、サイドバーでBotを開き、メッセージで仕事を渡すことです。入力にはファイル添付、音声、`@`での参照、`/`でのスキル指定があり、実行中の追加指示も受け付けます。会話にはツール操作、質問、承認、作成ファイルが並びます。結果への返信から修正を頼み、必要なら別のBotへ仕事を渡せます。[会話と共同作業](https://docs.x.ai/grok-bot/chat-and-collaboration)

| 利用場面 | Grok Botの公開UX | eneの現在の実装 | 評価 |
|---|---|---|---|
| 仕事を頼む | Botとの自然な会話から開始 | Companionのモデル出力を内部コマンドへ変換し、Taskを作成・委譲 | 基本経路は近い |
| 入力資料を渡す | 添付、画像貼り付け、リンクなど | テキスト入力と、パスを入力して選択するWorkspace | 入力の幅に差がある |
| 仕事中に会話する | 実行中にも追加メッセージを送れる | Task AgentをHostで別実行し、Companionとの会話を継続 | 基本経路は実装済み |
| 指示変更・停止 | 会話で方向修正や停止を指示 | 会話経由のsteering/cancelと、作業パネルの中止・再開 | 操作はあるが対象の示し方が弱い |
| 複数の仕事を扱う | 名前のあるBot、グループ、会話内の引き継ぎ | 複数Taskを保存できるが、会話の操作対象はCompanionごとに一つ | 実行の並行性と管理UXは別問題 |
| 状況に気づく | Working、Needs attention、未読の区別と通知 | 作業一覧と明示的な更新。中断の表示や未伝達結果のGUI接続に不足 | 差が大きい |
| 結果を見る | 会話のファイル・画像・リンクカードからプレビュー | 目的・結果テキストと「成果物・操作」の行 | 成果物の確認導線が不足 |
| 外部アプリで作業する | クラウドPCのブラウザ・端末・コネクター | Task Agentの道具はWorkspace内のlist/read/create/edit | 現在の実行範囲は限定的 |
| 操作を承認する | 会話上の操作・入力値を確認して許可/拒否。送信下書きも編集可能 | 権限と第一者確認の基盤はあるが、Task会話内に同等の操作・下書きカードはない | 基盤の存在と承認UXを分けて評価 |
| 画面を閉じる | クラウドで継続し、ノートPCを閉じても動作 | Client切断後もHostだけで進められる仕事を継続 | Hostが稼働している範囲で近い |
| 同じ仕事を繰り返す | 成功した手順をSkillにし、Routineを作成・テスト・管理 | Scheduleは次のStage 8。skill learningも後続計画 | 未実装の計画範囲 |
| 外出先で確認する | モバイルから同じBot・会話・結果・承認にアクセス | remote ClientはStage 14の計画 | 未実装の計画範囲 |

表のGrok Bot側は、上記の会話資料に加え、[ファイルと成果物](https://docs.x.ai/grok-bot/files-and-results)、[状態表示と通知](https://docs.x.ai/grok-bot/settings-and-notifications)、[コンピューターとアプリ](https://docs.x.ai/grok-bot/computer-and-apps)、[承認](https://docs.x.ai/grok-bot/approvals-security-and-privacy)、[SkillsとRoutines](https://docs.x.ai/grok-bot/skills-routines-and-automations)、[モバイル](https://docs.x.ai/grok-bot/mobile)に基づきます。ene側の根拠は以下のコード追跡とテストです。

会話から実行までの経路はつながっています。

```text
チャット入力
  → ene-companionが依頼を解釈
  → ene-core::HostTaskControl
  → ene-taskがTask作成・指示変更・結果採用を管理
  → ene-coreのlauncherがTask Agentをバックグラウンド実行
  → ene-action経由でWorkspace内のファイルを操作
  → ene-storeの保存済み事実を会話・作業パネルが参照
```

ユーザーに`[task-control]`を書かせる実装ではありません。Companionのプロンプトが自然言語の依頼を内部コマンドへ変換し、この内部表現は通常の表示・履歴から隠されます。作成、報告、指示変更、再開、中止が接続されています。[Companionの指示](/home/pexisgle/dev/Ene/crates/ene-companion/src/dialogue.rs:754)、[Hostでの委譲と起動](/home/pexisgle/dev/Ene/apps/ene-core/src/task_control.rs:408)

`ene-task`の責務はタスクの意味と保存境界です。UI、OS通知、ブラウザ、外部アプリの機能がこのcrateにないこと自体は問題ではありません。比較対象には、実行を組み立てる`ene-core`、会話を扱う`ene-companion`、表示を担う`ene-desktop`と`ene-desktop-ui`を含めています。[crateの責務](/home/pexisgle/dev/Ene/crates/ene-task/src/lib.rs:1)

実装から確認できる主な差は次のとおりです。

1. **作業を名前で選び、会話の対象を把握する体験が弱い。** 一覧のタイトルは目的の要約ではなく「作業1」「作業2」です。一覧DTOにある`purpose`も目的本文ではなく識別子です。会話のreport/steer/cancel/resumeには対象IDがなく、HostはCompanionごとの現在のTaskへ解決します。パネルで選べば対象を切り替えられますが、その選択が次の会話の対象になることを説明する表示がありません。Host再起動でこの一時的な対応は消え、改めて選択が必要です。Grok Botの名前付き担当者や会話から対象をたどる体験とは異なります。[一覧の生成](/home/pexisgle/dev/Ene/apps/ene-desktop/src/ui/tasks.rs:80)、[一覧DTO](/home/pexisgle/dev/Ene/crates/ene-api/src/v1/undelivered.rs:233)、[会話の対象管理](/home/pexisgle/dev/Ene/apps/ene-core/src/task_control.rs:143)

2. **動いていない作業を、一覧から見分けにくい。** 表示用の`rows()`は、実行登録があれば「実行中」、なければ保存済みのprogressを表示します。中断したTaskは`in_progress`かつ`running=false`なので「進行中」になります。内部には`is_interrupted()`がありますが、実際のカード表示には使われていません。再開ボタンも「Taskを選択したか」「指示を入力したか」を中心に有効化され、完了済みなどの判定は要求後のHost応答になります。Grok BotのNeeds attentionのように、ユーザーが何をすればよいかを一覧で判断する表示が必要です。[状態の組み立て](/home/pexisgle/dev/Ene/apps/ene-desktop/src/ui/tasks.rs:80)、[中断の判定](/home/pexisgle/dev/Ene/apps/ene-desktop/src/ui/tasks.rs:639)、[表示用語](/home/pexisgle/dev/Ene/apps/ene-desktop/src/ui/presentation.rs:80)、[再開操作](/home/pexisgle/dev/Ene/apps/ene-desktop-ui/ui/chat.slint:163)

3. **進捗・完了・不在中の結果を、通常GUIへ自動で届ける接続が不足している。** 作業一覧の取得は、接続時や「作業」「更新」などの操作時が中心です。workerの待機中に呼ぶ`tick()`はBody側の状態を扱い、Taskを更新しません。`present_task_undelivered()`と`ack_presented_tasks()`は実装されていますが、productionの呼び出し側を検索するとshellからの接続がなく、テストが直接呼んでいます。したがって、結果を保存・取得・提示確認する基盤があることを、GUIで完了報告が自動的に届くことと同一視できません。[GUI worker](/home/pexisgle/dev/Ene/apps/ene-desktop/src/shell.rs:723)、[更新処理](/home/pexisgle/dev/Ene/apps/ene-desktop/src/ui/runtime.rs:134)、[未伝達結果のAPI](/home/pexisgle/dev/Ene/apps/ene-desktop/src/ui/runtime.rs:1040)、[直接呼び出しているテスト](/home/pexisgle/dev/Ene/apps/ene-desktop/tests/stage7_c2.rs:488)

4. **成果物・操作のカードに、判断に必要な情報が届いていない。** Hostのreport DTOはaction attemptについて`source=None`を返します。TaskPanelはsourceがない行を空本文のカードへ変換するため、見出し「成果物・操作 N」だけになり、操作種別・対象ファイル・結果の確実性がそのカードに出ません。結果本文は表示できますが、成果物を開く、プレビューする、保存先を開くといった操作も接続されていません。会話の`TaskReport::render()`には変更ファイルと未確認作用があるため、保存済み事実と表示の間の不足です。[HostのDTO変換](/home/pexisgle/dev/Ene/apps/ene-core/src/presentation.rs:1966)、[カードの生成](/home/pexisgle/dev/Ene/apps/ene-desktop/src/ui/tasks.rs:570)、[詳細カードの描画](/home/pexisgle/dev/Ene/apps/ene-desktop-ui/ui/chat.slint:157)、[会話の結果報告](/home/pexisgle/dev/Ene/crates/ene-companion/src/dialogue.rs:1470)

5. **件数が増えると、一部の作業や結果へたどれない。** 一覧とreportはHost側でページ分割され、既定は50件です。しかしTaskPanelの`refresh_list()`と`load_report()`は先頭ページだけを取得し、返された`next_cursor`を扱いません。GUIにも「次のページ」がありません。51件目以降のTaskへこの一覧からアクセスできず、多数の操作履歴を持つTaskでは結果行が後続ページに回る可能性があります。なお、結果本文の分割取得を行う`load_source()`は次ページをたどっており、こちらとは区別します。[一覧取得](/home/pexisgle/dev/Ene/apps/ene-desktop/src/ui/tasks.rs:236)、[report取得](/home/pexisgle/dev/Ene/apps/ene-desktop/src/ui/tasks.rs:514)、[ページ上限](/home/pexisgle/dev/Ene/crates/ene-api/src/v1/undelivered.rs:53)

6. **依頼の受付や報告が、パートナーの日本語として整っていない。** 作業フォルダ未選択時は固定英語で選択を求め、受付も`Task accepted (status: in-progress)`です。報告には`result (adopted)`や`remaining/unconfirmed effects`などの内部処理に近い語が出ます。GUIのボタンは日本語化されていますが、会話側のTask制御応答は固定英語です。保存先の確認自体は必要でも、現在はフォルダパスを手入力するパネルへユーザーが移動して依頼をやり直す流れになります。[受付応答](/home/pexisgle/dev/Ene/apps/ene-core/src/task_control.rs:408)、[報告の生成](/home/pexisgle/dev/Ene/crates/ene-companion/src/dialogue.rs:1470)、[フォルダ入力欄](/home/pexisgle/dev/Ene/apps/ene-desktop-ui/ui/chat.slint:158)

7. **Grok Botの仕事の広さには、UI変更だけでは届かない。** Task Agentのプロンプトとrunnerの道具は、フォルダ内のlist/read/create/editに閉じています。シェル実行、Web検索、ブラウザ操作、外部サービスのコネクターは、この実行経路にはありません。「Webで調査し、表計算を更新し、連絡文を下書きする」といった横断作業を現在のTask Agentへ任せられるとは評価できません。Stage 4はファイル作業を対象としており、この制限は当該Stageの意図に沿っています。[道具の定義](/home/pexisgle/dev/Ene/crates/ene-task/src/agent.rs:637)、[Stage 4の範囲](/home/pexisgle/dev/Ene/docs/implementation/stages/stage-4.md:47)

eneで既に成立している点もあります。Taskの作成・指示変更・キャンセル・結果採用は、画面やモデルが直接状態を書き換える構造になっていません。古い指示と新しい指示、確定した変更と結果不明の作用、キャンセル受付と実行停止を分けています。Host再起動後の再開は明示指示を必要とし、結果不明の作用を勝手に再実行しません。この仕組みは、進行状況を安心して確認できるUXの土台になります。[Task状態の契約](/home/pexisgle/dev/Ene/crates/ene-task/src/task.rs:140)、[再開の要件](/home/pexisgle/dev/Ene/docs/requirements/requirements.md:199)

Grok Botを参考にしても、合わせる必要のない違いがあります。Grok BotのクラウドPCはアカウント内のBotでファイルやログインを共有します。eneは手元のHostとWorkspace・権限境界を重視しています。また、eneは雑談のタイムラインを作業ログで埋めず、Task管理を分けることを明記しています。会話の横に作業パネルを置く現在の方向は、この要件に合っています。Task Agentを名前付きの人格へ変えたり、すべての操作ログを会話へ流し込んだりすることは、この比較からは勧めません。[Grok Botの共有PC](https://docs.x.ai/grok-bot/computer-and-apps)、[eneの画面構成要件](/home/pexisgle/dev/Ene/docs/requirements/requirements.md:71)、[参考元との相違](/home/pexisgle/dev/Ene/docs/requirements/references.md:23)

改善の順序としては、まず「目的が分かる名前」「現在話しているTask」「中断・確認待ちの理由」「完了した成果物と残作業」を作業パネルにそろえ、既存の未伝達結果を通常のGUIへ接続するのが妥当です。会話には短い結果報告を置き、詳細は作業パネルへ誘導すれば、eneの雑談を保つ要件とも両立します。操作・結果のページ送りと日本語応答も、この段階で直せます。DTOに不足する情報や提示確認の契約を変える場合は、要件・設計に沿って変更を確定する必要があります。

その後に、既存ロードマップのSchedule、Observation、グループ会話、skill learning、remote Clientを進めます。添付、リッチな成果物プレビュー、会話内の承認カード、外部ツールの追加は、どの製品要件を満たすかを明示して設計する候補です。Grok Botに存在するという理由だけで実装仕様にはしません。[今後の実装順](/home/pexisgle/dev/Ene/docs/implementation/README.md:77)

検証では、通常のシェルにcargoがなかったため、既存の`nix-shell`開発環境を使用しました。次のテストはすべて成功しました。

| コマンド（nix-shell内） | 結果 |
|---|---|
| `cargo test --locked -p ene-task -p ene-companion` | 93件成功。doc testを含む |
| `cargo test --locked -p ene-core --lib task_control` | 29件成功 |
| `cargo test --locked -p ene-desktop --test stage7_c2` | 5件成功 |

合計127件です。会話からの作成・途中変更・完了報告、並行会話、切断後の継続、再開時の状態照合などの基盤を検証しています。ただし、これらはモデル応答を用意したテストを含み、実モデルが日本語の依頼を正しく振り分ける割合や、Slintの実画面で成果物を開けるかを保証するものではありません。特に、テスト用snapshotの`detail_text()`と実画面に渡る`rows()`/`details()`は別経路です。今回のUX差は、テスト結果に加えて後者の描画経路を追って判断しました。
