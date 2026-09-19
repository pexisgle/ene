# Credential の登録・更新・失効と有効世代の公開

本書は [IB K-C](interface-boundaries.md#k-c-秘密利用認証用途への供給)、[PR Group K](persistence-recovery.md)、[CCT](concurrency-control.md) を具体化する。OS 保護ストアの durable な秘密と、SQLite の非秘密参照・scrub 世代を、一つの DB transaction で更新できるとは仮定しない。適用対象は first-party の明示的な登録・更新・失効であり、登録は provider 利用同意を成立させない。

## 1. Owner と不変条件

- `ene-credential` が秘密の候補保存、有効 snapshot、scoped use、破棄を所有する。`ene-store` は非秘密参照・更新操作記録・既存の approval sweep / credential-set revision transaction を実装する。Host composition が配線し、GUI は owner を迂回しない。
- 有効値の対応は `(CredentialRef, SecretVersionId)`、本文の scrub premise は既存の `CredentialSetRevision` を使う。`SecretVersionId` はランダムな非秘密参照であり、秘密の hash・暗号化本文・認証能力ではない。別の currentness registry は作らない。
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
| 後処理対象 | 不採用または退役した version、cleanup 状態 | 他の active version を消す汎用パスを受け取らない |

phase は `Prepared`（候補 write の前）、`Staged`（OS 保存と照合済み）、`Activated`（有効参照と revision を commit 済み）、`CleanupPending`、`Completed`、`Abandoned` を区別する。有効化・失効の確定済み outcome と確定 revision は phase と独立に保持し、後処理や後続の更新で上書きしない。未採用 candidate の cleanup を終えても有効化成功にはならない。失効は候補 version なしで有効参照を無効化する。技術的失敗・OS write の成否不明も確定済み操作 outcome と分け、成功や「未実行」に丸めない。

## 3. 登録・更新の順序

1. Host は [first-party 確認](first-party-desktop.md#51-二つの-channel-と-owner-確認)の session を操作・credential identity・expected revision・mutation ID に束縛する。GUI の直接確認後だけ秘密 intake を行う。同じ mutation ID の再送は状態照会に収束させ、別の秘密を再投稿して上書きしない。入力し直す場合は新しい session / mutation ID が必要である。
2. HostLock を持つ serving Host が `Prepared` を durable に残してから、新しい version の OS item へ `put` する。OS dialog / I/O 中は control admission、SQLite transaction、公開 lock を持たない。read-back を credential owner 内で入力と照合して `Staged` にする。成功が不明なら同じ item を照会し、未確認のまま `put` を自動反復しない。まだ旧 snapshot が有効である。
3. candidate と現行の登録値から次の immutable snapshot を準備する。秘密の読込・snapshot 準備に失敗したら切り替えない。candidate の保持・取消・期限切れは当該操作の責任であり、通常の Targeted Deletion に破棄を委ねない。
4. control admission → credential owner の公開 write guard → 既存 master 上の transaction の順で取得し、session の live 性・対象・expected revision・操作 phase を再照合する。受理した確認の消費、approval sweep、active version 参照、credential-set revision、`Activated` と非秘密 outcome を一緒に commit する。競合に負けた候補は `Stale`、確認の失効は再確認待ちであり、新しい前提へ自動で付け替えない。commit の失敗では有効値を変更しない。
5. 同じ guard を保持したまま、準備済み snapshot を commit 済み revision として公開する。この区間に OS I/O や新規 allocation を入れない。公開に失敗した Host は credential 利用を unavailable にして recovery を要求し、旧 snapshot を新 revision のものとして使い続けない。成功応答はこの公開後である。
6. 不採用 candidate と退役 version は後処理対象にする。有効参照、実行中の lease、遅延結果の秘密除去が必要な間は削除しない。cleanup の失敗は有効化の成否と区別して表示し、登録処理を再実行する理由にしない。

OS item の version 作成、active pointer の更新、旧 version の破棄を一つの原子的 rename と説明してはならない。各中間状態は復旧可能な記録に対応させる。read-only の設定閲覧はこの後処理を行わない。

## 4. 並行する scrub・利用と失効

取得順序は **control admission → credential publication guard → SQLite** とし、各処理が必要なものだけを取得する。確認の確定はこの順序で GUI close / session expiry と直列化する。scrubber は read guard の中で immutable snapshot の参照と revision を一緒に取得し、guard を解放して scrub する。公開 guard や SQLite を持って control admission を取り直さず、この区間で `.await` や OS / provider I/O を行わない。snapshot を取得できなければ fail closed。保存・送信 claim は従来どおり同じ master 上で revision の等値を比較する。

認証利用は credential owner 内で version / revision に束縛した lease を準備し、同じ revision の claim が成立した場合だけ利用する。claim 後に裸の `CredentialRef` から「その時点の最新キー」を取り直してはならない。lease は非秘密の不透明 handle とし、生値を public に返さない。provider I/O は公開 guard / DB transaction の外で行う。

claim の既存 attempt 記録には利用 version の非秘密参照を対応付け、credential owner の cleanup はその未完了利用・結果受入の記録も照合する。メモリ上の lease が消えたことだけで退役 item を削除しない。restart 後に遅延結果を受ける場合、旧 item は秘密除去専用に読み込み、認証利用へ再有効化しない。必要な旧値を読めなければ結果本文の受入・公開を unavailable とし、未除去の本文を保存しない。

- **更新 commit が先**: 旧 snapshot で用意した本文や lease の新規 claim は stale となり、保存・送信はゼロ。新 snapshot では新しい値も秘密除去される。
- **claim が先**: 元の version に束縛した already-started use として扱う。新 version へ付け替えず、取消・失効時は best-effort 停止する。結果の成否不明を自動再実行しない。遅延結果の受入には現行値に加えてその lease の旧秘密も除去し、現行の保存・提示 gate を通す。
- **二つの更新**: 同じ expected revision からの有効化は一つだけ。敗者の OS candidate が既に保存されていても有効にしない。
- **失効**: 有効参照の無効化、revision 前進、非秘密 outcome を同じ transaction で commit し、新規利用を止める。進行中処理と秘密除去用 lease を排出した後に OS item と snapshot を破棄する。物理消去が未確認なら `CleanupPending` であり「失効・消去完了」としない。外部 provider のキーを失効させたとは主張しない。

秘密を含み得る認証 request/header のコピーは当該 I/O 終了・取消までに限定し、終了時に破棄する。古い秘密を除去するための lease は遅延結果の受入・破棄まで保持し、通常 DTO や Task 結果へ持ち出さない。

## 5. crash / restart と読み取り

| 停止位置・障害 | 復旧時の扱い |
|---|---|
| `Prepared`、OS `put` の結果不明 | 記録した version だけを OS store で照合する。有効化しない。未確認なら unavailable。自動 `put` 再実行をしない |
| `Staged`、有効化 commit 前 | 旧 active version を維持する。session は Host restart で失効するため、新しい本人確認なしに candidate を有効化しない |
| commit 後、snapshot 公開または応答前 | 現行 active version から startup snapshot を再構築する。同じ mutation ID の照会は独立に保存した commit 済み outcome を返し、後続更新済みでも過去の操作を再適用しない。同じ mutation による revision 前進を重ねない |
| active item の欠落・読込拒否・store locked | 対象の認証利用と、全登録値の除去を証明できない scrub を unavailable にする。旧 key、env、別 provider へ fallback しない |
| 退役 item の削除失敗 | `CleanupPending` のままにする。現行 active version を削除したり、有効化を巻き戻したりしない |

serving 起動は HostLock 下で未完了操作と active version を照合し、読み出した値による既存 startup sweep と revision 更新を済ませてから snapshot を公開する。この startup 検証による revision 更新は、過去 mutation の再適用とは別である。OS ストアが外部変更されていても旧 revision の値としては公開しない。自動で provider を呼び出さない。通常の open / read-only query は照合結果と非秘密 phase を読むだけで、startup repair・sweep・cleanup を開始しない。

秘密値と version item は backup に含めない。復元された参照だけで古い version を再有効化してはならず、現在の OS store と trust の維持は PR Group K の規則に従う。

restore owner は credential 更新・cleanup を停止した上で、現在の有効 / 失効 version と未完了操作を、置換される DB の外にある当該 restore の recovery context へ durable に保存してから切り替える。この非秘密 context はバックアップから読み戻さず、当該 restore に束縛する。復元した `credential_ref` はその現在値に照合し直し、過去 version の item が残っていても存在確認だけで有効化しない。対応を証明できなければ再登録を要求する。crash recovery も同じ context を用い、照合・sweep・新 revision の snapshot 公開前に serving を再開しない。これは現在の認証状態を守るための照合であり、過去の未完了 mutation を再実行する手順ではない。

## 6. 実装 gate

Stage 7 A1 で以下を、実 DB と制御可能な OS-store adapter の fault injection で検証する。実 OS adapter の put / get / delete / lock / restart は B の初回セットアップを有効にする前に両 OS で検証する。

- 旧 revision で本文・lease を準備した後に更新を止める barrier を置き、claim と有効化の両順序を検証する。新キーの本文漏洩、旧 claim による新キー利用はゼロ。
- 二つの更新、更新と失効、確認期限切れ・GUI 切断と有効化を競合させ、同じ revision の勝者が一つであることを確認する。
- OS `put` 前後、read-back、DB commit 前後、snapshot 公開前、応答前、cleanup 中の停止から、上表の状態へ収束することを確認する。
- OS store の外部変更が稼働中 snapshot を変更しないこと、再起動後は sweep と revision 更新前に利用されないことを確認する。
- 更新前に開始した provider の遅延結果が旧秘密を反射しても、更新・失効後の履歴・Memory・Task・提示・error に露出しないことを確認する。
- restart 後も未完了 attempt の旧 version を cleanup しないこと、旧 version が残る backup を restore しても現在の更新 / 失効を巻き戻さないこと、restore の切替前後の crash で同じ現在値へ収束することを確認する。
- mutation retry / restart で同じ mutation の二重有効化・二重 revision 更新・provider 自動呼出しがないこと、後処理や後続更新でも確定済み outcome を失わないこと、read-only query が状態を変更しないことを確認する。
