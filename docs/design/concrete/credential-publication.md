# Credential の登録・更新・失効と有効世代の公開

本書は [IB K-C](interface-boundaries.md#k-c-秘密利用認証用途への供給)、[PR Group K](persistence-recovery.md)、[CCT](concurrency-control.md) を具体化する。OS 保護ストアの durable な秘密と、SQLite の非秘密参照・scrub 世代を、一つの DB transaction で更新できるとは仮定しない。通常の登録・更新・失効は first-party の明示操作であり、登録は provider 利用同意を成立させない。登録秘密そのものを対象とする Targeted Deletion の参加は §4.1 の別契約とする。

## 1. Owner と不変条件

- `ene-credential` が秘密の候補保存、有効 snapshot、scoped use、破棄を所有する。`ene-store` は非秘密参照・更新操作記録・既存の approval sweep / credential-set revision transaction を実装する。Host composition が配線し、GUI は owner を迂回しない。
- 有効値の対応は `(CredentialRef, SecretVersionId)`。provider result を利用する Dialogue / Learning / Task Agent の本文は、現行 `CredentialSetRevision` に加え、由来となる ticket の durable な inference claim が使用した version 集合に scrub 証明を結合する。Task Agent final answer は単一 ticket ではなく、その delegation の 0..N 全試行の集合を対象にする。`SecretVersionId` はランダムな非秘密参照であり、秘密の hash・暗号化本文・認証能力ではない。別の currentness registry は作らない。
- OS item は installation namespace と version ごとに作り、公開済み item の値を上書きしない。credential label を同じ OS item への上書きキーにしてはならない。
- 新しい秘密は、既存本文の sweep、有効 version 参照の切替、revision の前進が同じ SQLite transaction で commit されるまで認証に使えない。OS `put` 成功は登録完了ではない。
- `ene-credential` は revision とその有効値をまとめた immutable snapshot を所有する。稼働中の通常利用では OS item を毎回読み直さず、この snapshot を使う。外部からの OS item 書換えを同じ revision の新しい値として取り込まない。再起動時の読込は serving 前の検証・sweep を経由する。
- 公開済み snapshot の秘密は credential owner の zeroizing メモリにだけ保持する。これは認証・scrub に必要な寿命であり、GUI、通常 DTO、DB、log へコピーする許可ではない。現行公開を退き、その世代を使う処理もなくなった snapshot は破棄する。

## 2. 非秘密の durable 記録

PR Group K の同じ owner に次を追加する。具体的な Rust 型名は実装で決めてよいが、相関と状態の意味は保持する。

| 記録 | 必須内容 | 秘密を含めない条件 |
|---|---|---|
| 有効 credential 参照 | credential identity、active version、有効性、既存 set revision | OS item の参照だけ。秘密値・値の hash は保存しない |
| 更新操作 | mutation ID、操作種別、credential identity、expected revision、旧/候補 version、phase、確認対象との対応、確定済み操作 outcome | 要求の fingerprint に秘密本文・その digest を入れない |
| 後処理対象 | 不採用または退役した version、cleanup 状態。退役 version は一件ずつ durable に記録し、後続の更新が pending を上書きしない | 他の active version を消す汎用パスを受け取らない |

phase は `Prepared`（候補 write の前）、`Staged`（OS 保存と照合済み）、`Activated`（有効参照と revision を commit 済み）、`CleanupPending`（この操作が退役させた version の消去が未確認）、`Completed`（この操作が退役させた version がすべて消去済み）、`Abandoned` を区別する。有効化・失効の確定済み outcome と確定 revision は phase と独立に保持し、後処理や後続の更新で上書きしない。未採用 candidate の cleanup を終えても有効化成功にはならない。失効は候補 version なしで有効参照を無効化する。技術的失敗・OS write の成否不明も確定済み操作 outcome と分け、成功や「未実行」に丸めない。

## 3. 登録・更新の順序

1. Host は [first-party 確認](first-party-desktop.md#51-二つの-channel-と-owner-確認)の session を操作・credential identity・expected revision・mutation ID に束縛する。GUI の直接確認後だけ秘密 intake を行う。同じ mutation ID の再送は状態照会に収束させ、別の秘密を再投稿して上書きしない。入力し直す場合は新しい session / mutation ID が必要である。
2. HostLock を持つ serving Host が `Prepared` を durable に残してから、新しい version の OS item へ `put` する。OS dialog / I/O 中は control admission、SQLite transaction、公開 lock を持たない。read-back を credential owner 内で入力と照合して `Staged` にする。成功が不明なら同じ item を照会し、未確認のまま `put` を自動反復しない。まだ旧 snapshot が有効である。
3. candidate と現行の登録値から次の immutable snapshot を準備する。秘密の読込・snapshot 準備に失敗したら切り替えない。candidate の保持・取消・期限切れは当該操作の責任であり、通常の Targeted Deletion に破棄を委ねない。
4. control admission → credential owner の公開 write guard → 既存 master 上の transaction の順で取得し、session の live 性・対象・expected revision・操作 phase を再照合する。受理した確認の消費、approval sweep、active version 参照、credential-set revision、`Activated` と非秘密 outcome を一緒に commit する。競合に負けた候補は `Stale`、確認の失効は再確認待ちであり、新しい前提へ自動で付け替えない。commit の失敗では有効値を変更しない。
5. 同じ guard を保持したまま、準備済み snapshot を commit 済み revision として公開する。この区間に OS I/O や新規 allocation を入れない。公開に失敗した Host は credential 利用を unavailable にして recovery を要求し、旧 snapshot を新 revision のものとして使い続けない。成功応答はこの公開後である。
6. 不採用 candidate と退役 version は後処理対象にする。退役 version は active 参照の移動と同じ transaction で一件ずつ durable に記録し、後続の更新は先行する pending を上書きしない。cleanup は bounded な pass で行い、OS item の削除を確認した transaction で記録を消して当該操作を `Completed` にする。crash や削除失敗では記録を pending のまま残して再試行し、消去未確認の version を消去済みとして報告しない。有効参照、実行中の lease、全消費主体の未完了 result 受入・stream 送信に必要な ticket / delegation、または後日に閲覧可能な保存済み Dialogue / Learning / Task intermediate / final 本文がこの version の秘密除去を必要とする間は削除しない。各本文 owner の ticket / delegation → 閲覧可能本文の索引付き bounded page を用い、保存・retention・消去による閲覧可能性変更と削除可否を同じ master の短い cleanup fence で直列化する。OS I/O 中に transaction を保持せず、fence 下で新たにこの item を要する保存・送信・表示は保留する。`unknown` や由来欠落で旧参照を同定できない item は隔離保持する。子行ゼロ、lease の消失、provider I/O の終了、Task seal や result 採用だけで必要性の消滅を推定しない。cleanup の失敗は有効化の成否と区別して表示し、登録処理を再実行する理由にしない。

OS item の version 作成、active pointer の更新、旧 version の破棄を一つの原子的 rename と説明してはならない。各中間状態は復旧可能な記録に対応させる。read-only の設定閲覧はこの後処理を行わない。

## 4. 並行する scrub・利用と失効

取得順序は **control admission → credential publication guard → SQLite** とし、各処理が必要なものだけを取得する。確認の確定はこの順序で GUI close / session expiry と直列化する。scrubber は read guard の中で immutable snapshot の参照と revision を一緒に取得し、guard を解放して scrub する。公開 guard や SQLite を持って control admission を取り直さず、この区間で `.await` や OS / provider I/O を行わない。snapshot を取得できなければ fail closed。provider result の現在の保存・表示境界では、同じ master 上で revision と由来 ticket の verified used-version set（Task Agent final は delegation 全体）を sealed proof と照合する。stream は chunk ごとに実送信開始の直前に照合する。旧 item の OS 読込は transaction 外で行う。

認証利用は credential owner 内で version / revision に束縛した lease を準備し、同じ revision の claim が成立した場合だけ利用する。claim 後に裸の `CredentialRef` から「その時点の最新キー」を取り直してはならない。lease は非秘密の不透明 handle とし、生値を public に返さない。provider の接続・TLS 準備と後続 I/O は公開 guard / DB transaction の外で行うが、認証 request の最初の送信には §4 の共有 gate 下で current lease / revision・消去条件・cap 等を照合した後の有界 nonblocking first-write を要する。claim commit や transport enqueue だけを利用開始としない。

claim は `inference_attempt` の `verified / unknown` と `(ticket, credential_ref, secret_version_id)` 子行を同じ commit で記録する。`verified` は lease の全参照を記録済みと証明する状態であり、認証情報を使わない verified ticket だけが空集合を証明する。V43 から移した `unknown` は子行がゼロでも空集合ではない。各 provider result の scrub は由来 ticket の verified 集合を同じ master から ticket-key の bounded read で取得する。Task Agent final answer は delegation の 0..N 全 inference attempts を索引付き bounded page で列挙し、各行の verified 性を確認してから集合を導く（試行が存在しない delegation のみ空集合）。複数 ticket の出力を一つの本文に組み込むときも由来の全 ticket を覆う。credential owner は現行 snapshot の値と集合に含まれる旧 OS item を秘密除去専用に読み、一つの論理本文全体を scrub して `ScrubbedText` に現行 `CredentialSetRevision` と対象・used-version set の proof を封止する。単なる現行値だけの scrub 証明を provider result の証明へ流用しない。旧値・その hash は SQLite、通常 DTO、Task 結果、監査へ置かず、旧 item を認証利用へ再有効化しない。`unknown`、必要な旧 item の欠落・読込拒否、proof の確定不能は本文の保存・提示なしの明示的な unavailable domain outcome とし、利用実績・既存の外部作用の事実は保持して再実行しない。

Dialogue / Learning / Task Agent intermediate の保存・再読込表示と各 stream chunk の送信前 gate は、現在の credential-set revision と当該 provider result の ticket 集合を同じ master 上の短い比較点で proof と照合する。stream の chunk 境界で分割された秘密を見逃さないよう、credential owner が論理本文の未送信 suffix を保持し、前後をまたぐ秘密を除去できた確定済み prefix だけを封止する。必要な保留長や bounded な連続照合を保証できなければ逐次送信せず最終本文の scrub まで保留する。生 provider chunk は Client の application queue・transport buffer・wire のいずれにも渡さない。

scrub 済み chunk の enqueue は送信許可ではない。Host の送信 owner は application queue、書込待ち、codec / transport buffer を含む全段階で未送信の項目に proof と由来 ticket / delegation の相関を保持し、各 chunk が実際に送信を開始する最終点で **credential publication guard → 同じ master の短い SQLite 比較点** を通す。現在 revision と verified used-version set を proof に照合し、削除対象の本文なら current erasure condition / phase を同じ送信前 gate に合成する。Client の対象・接続と当該部分の配送可能性を本文より先に durable commit し、commit 失敗時は 0 byte とする。送信開始前の再比較で drift したキュー項目は transport へ出さず破棄し、元の論理本文と連続 scrub 状態から現行値・必要な旧値を使って bounded に再 scrub するか unavailable として当該 stream を閉じる。既送信 prefix の proof を新 revision へ付け替えず、旧 stream の残りを新しい proof の chunk として継ぎ足さない。

credential 更新の SQLite commit と各 chunk の実送信開始を、同じ短い publication gate と同じ master の単一 writer の下で直列化する。更新・失効・削除開始の commit を行う書き手もこの gate を必ず取得し、別 writer が DB だけを更新したり送信側がプロセス内 mutex だけで比較したりする構成は許さない。送信側は事前に connection / TLS と背圧を ready にし、guard 下で現在 revision・verified used-version set・消去条件・対象接続を同じ master で比較して当該部分の配送可能性と outstanding delivery を durable commit する。SQLite transaction はここで解放し、guard を保持したまま当該 connection の単一の順序付き writer が、有界の同期・nonblocking first-write（必要な TLS record の生成と対応する ciphertext の OS への書込を含む）を実行する。正の byte 受理または実際に開始した byte の取り消せない handoff が確認できた点だけを送信開始とし、その後に guard を解放する。単なる application / TLS buffer への enqueue、書込予約、TLS ライブラリ内だけの平文受理、将来の flush は開始ではない。TLS record の準備だけが成功し OS 側で未開始なら、ready 扱いにしない。

接続・TLS handshake・背圧待ち、書込可能待ち、後続の network I/O と物理配送完了は guard 外で行う。guard 内で SQLite transaction を保持したまま I/O せず、`.await` や blocking OS / TLS I/O も行わず、上記の有界 nonblocking first-write だけを例外とする。EAGAIN 等で 0 byte の未開始を証明できれば guard を解放して待ち、再試行では配送可能性の既存記録を現在条件と照合し直してから書く。結果が不明なら元 chunk / ticket の `Unknown` と outstanding delivery を保持し、自動再送しない。正の部分書込が始まった場合はその prefix を元の revision / source / Client に帰属させ、残りは未開始として次の gate を通す。TLS 内の未書込 ciphertext が guard 外で自動 flush される構成は使わず、残りを再 gate できなければ connection を閉じて未開始 byte を送信不能にする。送信後の credential 更新・削除開始は既開始 prefix を引き戻したことにせず best-effort で停止し、到達不明を未送信と推定しない。更新 commit が先なら旧 proof の未開始 byte は 0、first-write が先なら既開始事実を保持する。これは送信開始の順序であり物理ネットワーク到達の保証ではない。

AU15a は結果受入・execution seal 前の同じ短い `Immediate` で現在の revision、当該 delegation の verified な全 used-version set、`ScrubbedText` の証明と到着引数を照合する。更新または新 claim が先に確定して不一致なら本文も seal も書かず、未確定の本文だけを現行値と必要な旧値で bounded に再 scrub / 再提出する。stream も stale chunk を送らず再 scrub か unavailable へ進める。上限到達時は domain refusal とし、provider 利用実績や既存の外部作用を自動再実行しない。確定済み result ID の retry は既存行の disposition に従う（IB の AU15a retry 契約）。

- **更新 commit が先**: 旧 snapshot で用意した本文や lease の新規 claim は stale となり、保存・未開始 chunk の送信はゼロ。新 snapshot では新しい値も秘密除去される。
- **claim が先**: 元の version に束縛した attempt / used-version 相関を削除参加と結果受入のため保持するが、claim 自体を実送信開始や旧 lease の将来の送信許可としない。更新・失効が first-write より先なら最終 gate で旧 lease の未開始 byte を 0 にし、既に first-write が先だった分だけ元の version に帰属する開始済み利用として best-effort 停止する。開始の成否不明を未送信と推定せず、自動再実行しない。遅延した Dialogue / Learning / Task Agent intermediate は由来 ticket、Task Agent final answer は delegation 全試行の旧 version の秘密も現行値とともに除去し、それぞれ現在の保存・提示 gate を通す。
- **二つの更新**: 同じ expected revision からの有効化は一つだけ。敗者の OS candidate が既に保存されていても有効にしない。
- **失効**: 有効参照の無効化、revision 前進、非秘密 outcome を同じ transaction で commit し、新規利用を止める。秘密除去用 lease と未完了受入・stream に加え、保存済みの閲覧可能本文に旧値が必要な間は OS item を保持し、公開 snapshot の認証利用は止める。全ての必要性が同じ cleanup fence で終了した後に item を破棄する。物理消去が未確認なら `CleanupPending` であり「失効・消去完了」としない。外部 provider のキーを失効させたとは主張しない。

秘密を含み得る認証 request/header のコピーは当該 I/O 終了・取消までに限定し、終了時に破棄する。通常更新・失効では、旧 item と非秘密 used-version 相関を全消費主体の未完了 result / stream および保存済み閲覧可能本文の現在表示に必要な間保持し、生値を通常 DTO や Task 結果へ持ち出さない。対象登録の Targeted Deletion は §4.1 に従って本文を消去または body-free unavailable に固定して必要性を閉じ、item の消去を先送りしない。

### 4.1 登録秘密を対象とする Targeted Deletion

保全・消去 owner が `ExactText` の登録済み秘密値そのもの、または `RegisteredCredential { ref }` の登録完全抹消を対象に確定する場合、`ene-credential` は required participant として参加する。漏洩コピーのみの削除では登録を失効させない。`ExactText` では認証秘密 owner が OS store 内の登録値を限定的に照合して該当 identity を確定する。`RegisteredCredential` では Client から非秘密の `CredentialRef` だけを受け、owner が OS store の該当登録の対象 version 値を機械的照合条件として解決する。いずれも同値の複数登録と各接続への影響、再認証の必要性を、秘密の表示・Client DTO・永続 hash なしで確認する。登録を残せば対象値の全域残存 0 を検証できないときは範囲を再確認し、対象外の登録・version を推測で削除しない。タスクや Companion の参照消去を登録自体の削除指示と読み替えない。

Targeted Deletion は保全・消去 owner の目的・target variant・対象 version / 同値登録の範囲・影響を束縛した trusted first-party 確認と durable な operation によって開始し、認証秘密 owner は対象同一性を再照合して参加する。これは通常の Host-local 登録・更新・一般失効の mutation ID / expected revision / 確認 session を流用して失効を装う経路ではない。逆に通常の credential 失効確認を全域消去の確認とみなさない。Client の専用面の入力経路と Host で検証できる確認由来を区別し、requester、通常の Client payload、LLM、tool、OS store unlock は確認を代行できない。

削除開始では、同じ SQLite master の erasure condition / operation の durable commit と対象 credential の利用禁止・非秘密参照の無効化・credential-set revision の前進、対象 version の全候補・退役 item の消去待ち相関を不可分に確定し、publication guard 下で新しい利用不可 snapshot を公開する。取得順序は control admission → credential publication guard → SQLite とし、開始を通常の更新公開、新 lease・claim、認証 dispatch / provider first-write の最終 gate と直列化する。削除開始の書き手も §4 の共有 gate と単一 writer を迂回しない。削除が先なら新規利用・未開始 byte は 0、first-write が先なら既開始 prefix を元の ticket に帰属させて best-effort 停止し、利用量・作用の Unknown を維持する。開始前に取った lease や古い snapshot を再利用するだけで gate を通らない経路を許さない。OS store I/O・外部 I/O や待機中に guard / transaction を保持せず、guard 内の送信開始は §4 の有界 nonblocking first-write に限る。commit 後の公開失敗や再起動は対象の利用を unavailable に保ち、消去条件・対象 item の照合と sweep を経るまで serving しない。失効済み参照・staged candidate・過去の cleanup 記録から対象 version を取りこぼさず、削除中の別 mutation が対象を再公開しないよう同じ境界で照合する。

通常 cleanup の「未完了 result / stream に必要なら退役 item を保持」の証明は、この operation の削除免除にはならない。各消費主体は対象 credential を使用した可能性のある verified / unknown ticket と delegation、保存済み provider-derived 本文・再読込表示・待機中の stream を列挙する。対象 version 値で全参加者の本文・派生データを機械的に scan・消去・残存検証し、登録の非秘密参照だけの sweep に縮退させない。対象 version の scrub が必要な本文は消去するか body-free unavailable として durable に固定する（Task final の保存済み `recorded` は `body_erased`、新しい到着は `erasure_held` の既存 disposition に従う）。元の帰属・seal・利用量・作用の確定度は保持する。未送信 chunk は application queue から transport 開始前まで止め、由来不明・`unknown` も空集合とみなさず同様に保留する。対象の秘密が後から反射された遅延 result や再接続 stream が復活しないよう、operation の現行条件と完了後の旧由来 cut で保存・提示・dispatch を拒否する。already-started 送信や provider I/O の停止・受信事実は正確に追跡し、外部側まで削除できたと主張しない。

登録値の機械的照合に必要な一時検証材料は、対象 item とは独立して消去できる OS secure store の operation 専用範囲だけに保持する。値・検索 token・hash を通常 SQLite、Client DTO、監査や participant fact に置かず、各参加者の照合には owner の限定的な秘密利用境界を用いる。対象値の全参加者 scan・残存検証、旧 in-flight の本文受入・送信の収束、`Finalizing` の新規本文受入停止を終えるまで、対象 item も一時材料も破棄しない。その後、対象 item の全 version（active / staged / retired / cleanup pending）と private snapshot / lease / request buffer を zeroize・削除し、独立した一時材料も消去する。OS delete と再照会、非秘密の current pointer・用途参照・対象本文を含み得る操作記録の消去または復元不能化を owner が検証し、対象外 credential の item と参照は維持する。削除中に必要な対象登録・version の対応は操作に束縛して保護し、局所検証後は対象参照そのものを残さず、body-free な旧 ticket / version の事実は消去対象の値や登録参照を復元できない最小限にする。削除済み item を秘密除去のために再取得したり、削除前の snapshot から再公開したりしない。crash 後の材料欠落・破棄成否不明、OS store の locked / unreachable、memory lease の未排出、残存・再到着防止の検証不能では current condition と受入停止を維持して pending / unavailable とし、必要なら Owner に再提供を求め、全域完了を返さない。通常更新の `Completed` や一般失効の `CleanupPending` は本 operation の局所 Verified の代わりにならない。外部サービスの credential 失効・更新は実行も完了宣言もしない。

この一時材料の OS 保存より前に、operation 専用 namespace / item ID / version と準備 phase を非秘密の durable preparation として書く。read-back 済みの値だけを deletion start へ渡し、開始 commit は preparation を operation の材料参照に原子的に移す。commit に失敗しても準備値を別の operation / 値へ再利用せず、確認の再送は既存状態の照会に限る。OS I/O 中は publication guard / SQLite transaction を持たない。serving startup は、通常の read-only open とは別の mutating reconciliation で同 namespace の全候補 item を列挙し、DB の未完了 operation・preparation と restore recovery context の ID / version に照合する。前回の準備 session は失効させ、operation の無い候補と孤立した準備 material は zeroize / delete / 再照会を確認してから正常な credential 利用と新規 deletion を許す。列挙不能・不明な item・delete 未確認は保留し、未完了 operation に紐づく材料の欠落は残存 0 や完了の証拠にしない。operation に結び付いた材料は全参加者の機械検証が有効な間は維持し、最終 wipe / 再照会までは局所 Verified にしない。

remote `ExactText` の文字列は照合前から登録秘密であり得るため、通常の削除 DTO には通さず、削除専用の短寿命 secret-bearing input frame から直接 owner の照合へ渡す。照合後も生値を通常 SQLite・Client DTO・log に置かず、登録との一致と用途・対象 version は非秘密参照だけで確認する。限定経路が使えなければ remote の値指定を拒否して ref 指定へ誘導する。これは通常 credential 登録・更新の Host-local 秘密 intake を変更しない。

## 5. crash / restart と読み取り

| 停止位置・障害 | 復旧時の扱い |
|---|---|
| `Prepared`、OS `put` の結果不明 | 記録した version だけを OS store で照合する。有効化しない。未確認なら unavailable。自動 `put` 再実行をしない |
| `Staged`、有効化 commit 前 | 旧 active version を維持する。session は Host restart で失効するため、新しい本人確認なしに candidate を有効化しない |
| commit 後、snapshot 公開または応答前 | 現行 active version から startup snapshot を再構築する。同じ mutation ID の照会は独立に保存した commit 済み outcome を返し、後続更新済みでも過去の操作を再適用しない。同じ mutation による revision 前進を重ねない |
| active / 結果除去や保存済み本文の後日表示に必要な退役 item の欠落・読込拒否・store locked、または V43 ticket の `unknown` | 対象の認証利用または provider result の scrub を unavailable にする。本文の保存・提示・残りの stream chunk 送信は行わず、利用実績・外部作用の事実を保持する。旧 key、env、別 provider へ fallback しない |
| 退役 item の削除失敗 | 記録を pending のままにし、次の bounded pass で再試行する。現行 active version を削除したり、有効化を巻き戻したりしない |
| deletion 用 preparation の OS 保存前後、開始 commit 前に停止 | mutating startup reconciliation で operation 専用 namespace の全候補を列挙し、operation に未接続の準備 item を消去・再照会する。不明 / 未確認なら登録秘密の正常利用と新規 deletion は保留。保存や開始を自動再実行しない |
| deletion 開始 commit 後、全域検証または material wipe 中に停止 | operation 参照に結び付いた材料を保護して再開する。欠落・wipe 成否不明なら current condition と本文受入停止を保ち、消去・検証の残りを証明できなければ完了としない |

serving 起動は HostLock 下で未完了操作と active version を照合し、読み出した値による既存 startup sweep と revision 更新を済ませてから snapshot を公開する。この startup 検証による revision 更新は、過去 mutation の再適用とは別である。OS ストアが外部変更されていても旧 revision の値としては公開しない。自動で provider を呼び出さない。通常の open / read-only query は照合結果と非秘密 phase を読むだけで、startup repair・sweep・cleanup を開始しない。

秘密値と version item は backup に含めない。復元された参照だけで古い version を再有効化してはならず、現在の OS store と trust の維持は PR Group K の規則に従う。

restore owner は credential 更新・cleanup を停止した上で、現在の有効 / 失効 version と未完了操作を、置換される DB の外にある当該 restore の recovery context へ durable に保存してから切り替える。この非秘密 context はバックアップから読み戻さず、当該 restore に束縛する。復元した `credential_ref` はその現在値に照合し直し、過去 version の item が残っていても存在確認だけで有効化しない。復元した ticket / delegation の used-version state と子行に加え、閲覧可能な保存済み本文の由来と旧 version の逆参照を照合する。必要な旧 item が現在の OS store にない、または state が `unknown` なら、遅延 result の本文保存・再読込表示を unavailable とし、復元済みの利用実績・外部作用 facts を変更しない。旧 item の存在だけで保存済み本文を自動公開せず、現在の proof を改めて照合する。対応を証明できなければ再登録を要求する。crash recovery も同じ context を用い、照合・sweep・新 revision の snapshot 公開前に serving を再開しない。これは現在の認証状態を守るための照合であり、過去の未完了 mutation を再実行する手順ではない。

## 6. 実装 gate

Stage 7 A1 で以下を、実 DB と制御可能な OS-store adapter の fault injection で検証する。実 OS adapter の put / get / delete / lock / restart は B の初回セットアップを有効にする前に両 OS で検証する。

- 旧 revision で本文・lease を準備した後に更新を止める barrier を置き、claim と有効化の両順序を検証する。新キーの本文漏洩、旧 claim による新キー利用はゼロ。
- 二つの更新、更新と失効、確認期限切れ・GUI 切断と有効化を競合させ、同じ revision の勝者が一つであることを確認する。
- OS `put` 前後、read-back、DB commit 前後、snapshot 公開前、応答前、cleanup 中の停止から、上表の状態へ収束することを確認する。
- OS store の外部変更が稼働中 snapshot を変更しないこと、再起動後は sweep と revision 更新前に利用されないことを確認する。
- 更新前に開始した provider の遅延結果が旧秘密を反射しても、更新・失効後の履歴・Memory・Task・提示・error に露出しないことを確認する。
- Dialogue / Learning / Task Agent intermediate の各 ticket と 1 delegation の複数 inference attempts が異なる旧 version を使った場合、それぞれの本文から必要な全旧値と現行値を除去し、保存・提示前と各 stream chunk の実送信開始点で revision と verified set を照合することを確認する。旧 revision の scrub 済み chunk を enqueue → 更新 commit → dequeue とし、application / transport のどの待機段階でも旧 proof の未開始 chunk が 0 byte となること、連続 chunk を跨ぐ秘密や生 provider chunk が Client に出ないことを検証する。逆順では開始済み chunk の利用・配送可能性を保持し、後続 chunk を再照合する。削除条件の送信前 durable 記録が失敗した場合も 0 byte とし、AU15a では本文・seal を書かず bounded 再 scrub になる。
- `verified` の認証情報なし ticket と、V43 由来の `unknown` で子行ゼロの ticket を区別すること、旧 item 欠落時は本文保存・提示を unavailable にして利用・作用 facts を残すことを確認する。restart 後も未完了の受入・stream と保存済み閲覧可能本文に必要な旧 item を通常 cleanup せず、retention 後に全参照がなくなった item だけを消す。backup restore と切替前後の crash で現行更新 / 失効を巻き戻さない。
- mutation retry / restart で同じ mutation の二重有効化・二重 revision 更新・provider 自動呼出しがないこと、後処理や後続更新でも確定済み outcome を失わないこと、read-only query が状態を変更しないことを確認する。
- Stage 6 の Targeted Deletion producer では、コピーだけの指定と登録値 / 参照の明示指定、同じ値の複数登録と無関係な登録、削除開始と更新 / lease / claim / dispatch の両順序、旧 ticket の verified / unknown と保存済み本文・未送信 chunk を検証する。対象全 version の OS delete 失敗・store lock・crash / restore 中は participant を未完了に保ち、削除と機械検証後は本文の再充填・旧参照の再公開を防ぎ、他の登録の認証利用を維持する。
- 一時材料の preparation durable 前後、OS `put` 前後、read-back 後、開始 commit 前後の crash を差し込み、未接続 item の列挙・消去検証が終わるまで serving と新 deletion が保留されること、operation へ移った材料は全域検証まで維持されること、削除失敗や材料欠落で完了にならないこと、retry が別の値を同じ準備へ流し込まないことを確認する。
