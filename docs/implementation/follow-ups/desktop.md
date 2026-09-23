# Desktop の会話・寿命・Body の後続計画

[実装ガイド](../README.md) / [Stage 7](../stages/stage-7.md) / [Stage 14 の入口](../README.md#42-機能と追跡先)

Stage 7 の統合済み機能と acceptance は [進捗](../PROGRESS.md) で判定する。この文書は残る製品経路と修正の作業順を定める。窓を閉じた後の Desktop / Body 寿命や remote 初回接続は現行 [First-party desktop](../../design/concrete/first-party-desktop.md) と食い違うため、要件・設計更新を先行する。D1 の具体的な表示・スクロール規則など、現行要件がまだ定めない製品挙動も、実装前に受け入れ条件へ反映する。

## D1: 初回設定の状態と会話画面（Stage 7 B）

1. Host が初回設定を確定した応答で GUI を通常画面へ遷移させる。失敗・保留なら設定画面に留めて理由と次の操作を示す。再起動時は Host の現在の設定事実から初期画面を選び、資格情報や同意の失効・利用不可を設定不足と区別して案内する。画面遷移や状態確認のための AI 呼び出しをしない。新規領域での確定直後と新しいプロセスでの再接続を、Windows / Linux の製品 GUI で確認する。
2. チャットを開き非同期に履歴を読み込んだ後は最新を表示する。末尾付近での新着、本文の高さ変化、window resize には追従し、本文を入力欄の下に隠さない。過去を読んでいる間は位置を保ち、「最新へ」で戻れるようにする。単なる履歴更新でも閲覧位置を飛ばさない。
3. 自分と ene の発言を配置・色・形・本文幅で読み分けられる会話表示にし、既存アバター・配色と整合させる。時刻は簡潔に見せ、完全日時も参照可能にし、日付境界を追えるようにする。短文と長文の行間・余白を整え、会話・作業・設定の区別を保つ。入力欄・送信ボタンの高さ、配置、focus、狭い画面とキーボード・日本語 IME の操作を確認する。接続中・応答待ち・送信不可・失敗を会話の流れで示し、重複送信を避ける。provider 障害の原因表示は [U4](usage.md#u4-チャットへ-provider-障害を伝える) に接続する。
4. 日本語・英語、空履歴、長文、狭い window、キーボード、IME、失敗状態を実 GUI で確認する。scroll と表示変更は同じ画面で統合検証する。

## D2: Body の固定フレーミング（Stage 7 D）

- 現フレームの mesh 外接範囲から毎回 center / zoom を算出する経路を原因候補として調べる。基準姿勢・アセットと配置箱に対する安定した framing を設計し、モーションの一時的な外接変化で視点を動かさない。大きなモーションの見切れ方も明示する。
- idle と複数 motion を連続再生し、基準位置・倍率が跳ねないことを確認する。Body の drag・resize と HiDPI を維持し、描画と alpha-aware input mask、透明点の click-through、アバター上の capture を一致させる。対象 OS・VRM・motion pack・clip を記録する。サンプルでの結果を公式 `ene` での最終確認に代用しない。

## D3: 公式アセット・物理 HiDPI・統合 acceptance（Stage 7 D/F）

- 権利確認済みの公式 `ene` VRM 1.0 を install asset として用意する。表情・humanoid animation・SpringBone が扱えることを確認し、Host の W-7 descriptor と Body がサンプルでなく公式アセットを参照する。退役実装のアセット移植、仮キャラクターの製品扱い、汎用 character editor の先行追加はしない。Windows 11 と対象 KDE Wayland でアセット ID と実表示・motion を記録する。
- 物理 output を scale=2 にした KDE Wayland で layer-shell placement、buffer scale、論理座標の input region、透明点とアバター上の入力、drag / resize、hide / restore、placement 更新、`wp_presentation` と FPS 30 以上を確認する。変更前の display 設定を記録し復元し、通常 scale も回帰確認する。入れ子 KWin の probe は物理 output の証拠にしない。
- WSS 統一、公式 VRM、Body 修正、対象 OS の正式環境を含む統合 tip で Stage 7 acceptance / performance を再判定する。旧 transport や別アセット・別 tip の probe を統合合格と読み替えない。

## D4: 窓・Desktop・Host の寿命（要件・設計 gate → 実装）

1. 要件と First-party desktop 設計で、窓を隠す、Body を一時非表示にする、Companion を止める、Desktop を終了する、Host を止める操作と Task・presence の寿命を分ける。単一トレイを Desktop が所有する案を検討し、現行の GUI close で Body を終了する契約を変更する範囲と実装 Stage を決める。隠した確認窓の `ConfirmationSession` と入力途中の秘密、空席時の本人確認を明記する。
2. 合意した寿命契約に従い、窓 close では窓を隠して Desktop / Body を稼働させ、トレイからチャット・設定を再表示する。明示的な Desktop 終了は Body を終了させ、Host-only Task は Host で継続する。トレイ不能な環境でも Host の状態確認・停止手段を残す。
3. Host 停止は新規受付を閉じ、実行中 Task・未確定の作用・確認保留・GUI / Body の終了順を設計してから実装する。結果不明を成功・未実行とみなさず、再起動で自動再実行しない。Host PC の正式 GUI からの停止と、単独起動 Host 用のローカル `ene-core status` / `stop` 相当を同じ Host 側契約へ接続する。status は read-only、CLI は高権限確認を代行しない。remote Client には Host 停止を出さない。
4. Host から起動した Desktop と単独起動 Host の両方で status / stop、窓の hide / restore、Body と Task の寿命、確認中の close、異常終了・再起動を検証する。Windows 11 と対象 Linux desktop でトレイを実機確認する。Host 自動起動は後続 Stage 9 と整合させる。

## D5: 起動時の接続先選択（設計後、remote 実接続は Stage 14）

- 要件・設計で bootstrap UI と Host 発行の first-party control seat を分離し、起動前の接続先選択、保存・変更、接続失敗時の戻り先、認証後の Body 表示条件を定める。local 選択後だけ既存 Host への接続または起動を行い、remote 選択でこの PC の Host を起動しない案を Stage 14 の認証・pairing 契約と整合させる。
- remote を選んだだけで接続・presence 成立とは扱わず、Host 検証と Host PC 側の承認を要する。鍵不一致・不通・切断で local へ自動切替しない。接続設定・pairing 画面は認証前から表示できるが、Body と接続済み会話は現在の接続・認証・presence の成立後に限る。remote Desktop に Host-local の直接確認席や Host stop を与えない。
- 初回選択 UI のみで remote Client 提供済みとはしない。Stage 14 の実接続時に local / remote、再起動後の接続先、失敗・鍵不一致・切断を両 OS で検証する。
