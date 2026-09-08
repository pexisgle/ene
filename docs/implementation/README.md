# ene 実装ガイド (Implementation Guide)

状態: **初版**（Walking Skeleton 着手用。Requirements / Architecture / Architecture Review #3 完了後）

> 前提: Requirements（製品定義・要件・受け入れ条件）と Design（Architecture / Critical Areas / Subsystem / Concrete）および Architecture Review #3 の統合が完了している（2026-09-08 時点）。本書は **Walking Skeleton 以降の実装段階**を対象とし、Requirements / Design 自体は変更しない。
本書は、`/goal` による長時間・自律的な ene 実装を進めるための**navigation document** である。既存の [Requirements](../requirements/README.md) と [Design](../design/README.md) を実装へ展開するときの手順・判断基準・検証方法を定める。本書は Design artifact ではなく、実装の source of truth は常に Requirements / Design 側にある。

## 1. 前提と正本の関係

- 製品挙動の source of truth は [`docs/requirements/`](../requirements/README.md)（`product.md` / `requirements.md` / `acceptance.md`）。
- 内部設計の source of truth は [`docs/design/`](../design/README.md)（architecture → critical-areas → subsystems → concrete の順に具体化される）。
- `reviews/` は判断履歴・検証記録であり、現行 Architecture の代替正本ではない。Architecture Review #3 の指摘（F-01〜F-07, RA-01）は既に concrete artifact へ統合済みであり、本Guide は統合後の設計だけを対象とする。
- 下位 artifact と上位 artifact に矛盾が見つかった場合は、下位側で意味を上書きせず **Issue として扱う**（[design README の precedence / conflict rule](../design/README.md)）。
- 既存 Requirements / Design は変更しない。変更が必要な問題は §7 の手順で Issue 化する。
- **既存コードは極力流用しない**。過去の実装（`.old/`）は Requirements / Design の正式な根拠ではなく、必要な場合の参考にのみ用いる。

## 2. 実装全体の規律（全 milestone 共通）

### 2.1 検証コマンド（[AGENTS.md](../../AGENTS.md)）

- 整形: `cargo fmt --all`（check: `cargo fmt --all -- --check`）
- focused iteration: `cargo check -p <pkg>` / `cargo test -p <pkg>`
- 全体 lint: `cargo clippy --workspace --all-targets -- -D warnings`
- 全体 test: `cargo test --workspace`
- docs: `cargo doc --workspace --no-deps`
- **注意**: `default-members = ["apps/ene-ctl"]` のため bare `cargo test` は workspace 全体を検証しない。全体確認には必ず `--workspace` を付ける。
- CI は [.github/workflows/ci.yml](../../.github/workflows/ci.yml) が fmt / clippy / feature matrix / workspace test / docs / windows-native を実行する。workspace clippy policy は厳格（`unwrap_used` / `panic` / `print_stdout` / `todo` / `unimplemented` 等が deny）。lint を弱めて通すのではなく実装を直す。
- Linux ネイティブ作業は Nix flake 前提（`direnv` 未使用時は `nix develop --command`）。Windows は MSVC + VS Build Tools + Windows SDK。

### 2.2 Scope control（スコープ制御）

- 「将来必要になりそう」という理由だけで先行実装しない。現在の milestone を成立させるために必要なものだけを実装し、後続機能は後続 milestone へ送る。
- 一時的な Architecture bypass や「後で必ず捨てる偽実装」で milestone を通過させない。薄い実装・単一 Provider 対応から始めてよいが、既存 interface / ownership / authority boundary は必ず通す。
- 旧実装（`.old/`）は退避済みであり、workspace の対象外。実装は**新規に行う**。旧コードを参考にするのは「どうしても必要な場合」だけとし、その場合も**十分なテストを伴って**参考にする（旧コードはバグを含む可能性が高いため、挙動を無検証で引き継がない）。
- 旧実装の `apps/ene-desktop` は frozen 扱い（`.old/`）。新実装では `crates/*` を新規に構築し、旧実装の crate 構成に依存しない。

### 2.3 自律判断してよい範囲（Design Freedom）

**旧実装 `.old/` の取り扱い**：既存の Rust workspace（crates / apps / plugins / templates / scripts / assets / third_party）は `.old/` へ退避済みで、新workspace からは除外されている（`.old/` は workspace の `members` 対象外であり、`exclude` に含める）。

- Requirements / Design が実装の source of truth。旧コードの挙動を要件へ昇格させない。
- 旧コードは「どうしても必要な場合」のみ参照する（例: 既存 asset 形式、外部 protocol の実装例、設計文書が旧実装を前提にした参照）。参照した場合は、その旨と範囲を PR に明記し、該当部分は必ずテストを書く。
- 旧実装との互換性・移行段階（旧 CM §13 の Stage 0〜5 や shim 維持）は実行しない。要件と新設計に基づき新規に構築する。

既存 Design が「意図的に残した Design Freedom」として列挙する項目は、実装者が合理的な方式を自身で選択してよい。人間の確認は要求しない。

- Rust type の細部（型名・method 名・module 粒度・crate 内構造）
- SQL column / index、migration の具体形（PR §4 の table group と owner 注記に従う範囲）
- library の利用方法、concurrency mechanism の選択（CCT §17 の三択：CAS / owner-local mailbox / 短 transaction 内 atomic read+insert）
- retry / timeout 値、費用予約量の算定式、観測頻度・scheduling algorithm、prompt 組立、cache 実装
- test の実装内容（検証すべき contract は Design が定めるが、test 自体は自由）
- performance 上の局所判断、同期 / 非同期の粒度
- Provider / MCP / Plugin adapter の concrete API（trait の boundary contract は維持する）

逆に、各 artifact が「固定前提（fixed premise）」として宣言するもの（semantic owner、identity / revision / generation の分離、serialization domain、caller ≠ authority、secret 非返却、保存分類、IPC の Host-local / remote-capable 区別等）は変更・再解釈しない。

### 2.4 実装中に守るべき共通の重要点

- **caller ≠ authority**: 呼べたこと・Client message が届いたこと・Task Agent result が到着したことを成立・確定にしない（IB §2）。
- **compare-before-commit**: current 条件（typed expected revision / presence generation / restore generation）を短 transaction で atomic に比較してから commit する（CCT §5）。
- **secret 非返却**: `SecretValue` を public に返さない。wire・context・History・Learning・log・Debug capture へ平文を出さない（DR-05）。
- **Transaction 非露出**: `rusqlite::Transaction` や生 SQL を business layer / Client / Provider / MCP へ露出しない（PR §7）。
- **保存分類**: durable（app.db）と derived（derived.db / sqlite-vec / index / cache）と external（E）の区別を保ち、derived を正本にしない（PR §3・§5）。
- **通常の忘却と削除の区別**: 忘却・訂正・失効・置換・統合は保存済み Learning / revision を削除しない。削除は Privacy/Security 目的の targeted deletion と、Owner 明示の容量管理 cleanup に限る（requirements「重要度、忘却、訂正」）。
- **Host 正本 / Client 一時 data**: Client に永続正本を作らない。Client は `ene-api` のみに依存し、Host domain crate・`ene-store`・secret に直接依存しない（CM §11、IPC 原則）。

## 3. Walking Skeleton の定義

この Project の Walking Skeleton は「テキスト timeline と一対一会話」「Companion scope の Experience Summary / Memory 形成」「一つの Workspace folder を対象とする Task Agent 委任の file 作業」「Client 不在時の Task 継続」「Host 再起動後の明示再開」「presence 復旧」「Text 管理面」を縦断 slice として同時に成立させる段階である（[CM §13](../design/concrete/crate-module-decomposition.md) の定義に従う）。Walking Skeleton は単一 milestone ではなく、M1〜M5 を通じて段階的に構築され、M6〜M7 で acceptance の範囲を満たす。

Walking Skeleton 成立の判定は、[acceptance.md](../requirements/acceptance.md) の代表シナリオ（Setupと最初の会話 / 会話からのMemory / Workspaceでのfile Task / ClientとHostのlifecycle / 障害と安全境界 / 利用量と秘密情報）の主経路が、Support Matrix の両 OS で再現可能な手順により完了することである。性能 Gate と baseline は M7 で判定する。Voice・Observation・Schedule・Remote Client・backup からの復元・Skill 生成・Character 配布・Global scope・Companion State・Relationship は Walking Skeleton に含めない（M8 以降）。

## 4. Milestone の全体像と dependency

milestone は依存関係と Architecture validation の観点から区切る。開始順に、**M0 退避・workspace 再構成 → M1 基盤 crate 新規構築 → M2 テキスト会話の縦断 slice → M3 Learning（Summary / Memory）→ M4 Task Agent file 作業 → M5 Client 不在時継続・presence 復旧 → M6 Targeted Deletion・容量管理・Audit → M7 管理面・Body・性能 Gate・両 OS 受け入れ → M8 以降（後続機能）** とする。

| ID | 名前 | 依存（先に必要） | 並行・入替可能 |
|---|---|---|---|
| M0 | 旧実装退避と workspace 再構成 | — | — |
| M1 | 基盤 crate の新規構築（primitive / config / error） | M0 | — |
| M2 | テキスト会話の縦断 slice（round・保存・Host/Client 境界） | M1 | — |
| M3 | Experience Summary / Memory 形成（Learning） | M2 | M4 と入替可 |
| M4 | Task Agent 委任 file 作業（Workspace） | M2 | M3 と入替可 |
| M5 | Client 不在時継続・Host 再起動・presence 復旧 | M3, M4（Task 継続と未伝達報告を含むため） | — |
| M6 | Targeted Deletion・容量管理・Audit | M3, M4, M5 | — |
| M7 | 管理面・基本 Body・性能 Gate・両 OS 受け入れ | M2〜M6 | Body は Client 側で M4 と並行開発可 |
| M8 | Schedule | M4 | — |
| M9 | Backup / Restore / Update・自動起動 | M6, M7（Credential store・Audit 契約が前提） | — |
| M10 | Voice | M7（presentation 基盤） | — |
| M11 | Observation / Observer | M4（Task 基盤）, M5（presence）, M7 | — |
| M12 | グループ会話・Companion 間交流・自発性 | M3, M5 | — |
| M13 | Global scope・Skill 形成改善・Companion State・Relationship | M3 | — |
| M14 | Remote Client（pairing / device 失効 / 移動） | M5, M7, M9 | — |
| M15 | Character 編集・Package 配布 | M1（ene-character 拡張） | — |
| M16 | 複数 Provider・fallback・費用 cap 完成 | M6, M7 | — |

順序を調整してよい範囲: M3 と M4 の入替（acceptance は Memory を先に検証する想定だが、依存上はどちらも M2 のみ必要）。M7 の Body は Client 側実装のため Host 側 milestone と並行可能。M8 以降は各機能の依存を満たせば順序を入れ替えてよい。

## 5. Milestone 詳細

各 milestone に「目的 / 完了状態 / 次へ進む前の validation / この段階で実装しなくてよいもの / 実装中の注意点」を記す。task checklist は意図的に置かない。実装者は Design artifact を読み、自身で具体作業へ分解する。

### M0 — 旧実装の退避確認と workspace の再構成

- **目的**: 旧実装（`.old/`）の退避状況と、新規 workspace の構成（`Cargo.toml` の members / exclude）を確認し、実装の土台を整える。旧コードへの依存を遮断する。
- **完了状態**: `.old/` に旧実装一式が退避され、workspace から除外されている。`cargo test --workspace` が空集合で green（または最小の新規 crate のみ）、`cargo clippy --workspace --all-targets -- -D warnings` が green。
- **次へ進む前の validation**: workspace 全体の test / clippy / fmt green。旧実装の crate が新 workspace に含まれないことを `cargo metadata` で確認。
- **実装しなくてよいもの**: 新機能、リファクタリング、旧実装の移行・shim 維持、旧 crate への依存追加。
- **注意点**: `.old/` は reference として保持するが、挙動の source of truth にしない。Requirements / Design に基づき新規構築する。

### M1 — 基盤 crate の新規構築（primitive / config / error）

- **目的**: 新規実装の第一歩として、依存の葉から crate を構築する。`ene-primitive` を新設し（`RawId` / `RevisionInner` / `GenerationInner` / `WallClockWithTz`）、`ene-config`・`ene-error` 等の基盤 crate を定義する。旧実装の `ene-card` 等は参照しない。
- **完了状態**: 新規 crate が workspace に追加され、`cargo test --workspace` / clippy / fmt green。依存方向が CM §4 の方向（葉 → domain）に従う。
- **次へ進む前の validation**:
  - `cargo test --workspace` / clippy / fmt green。
  - CM §12 の crate 依存 walkthrough のうち、この段階で成立する crate hop を確認（`ene-primitive` への依存追加が leaf 方向のみであること）。
  - CI（feature matrix 含む）green。
- **実装しなくてよいもの**: `ene-store` 新設、`ene-presence` / `ene-task` / `ene-learning` の本格新設、wire / IPC、domain 挙動。
- **注意点**: `SecretValue` を public に返さない。`Transaction` を公開しない。旧実装の型・crate を流用せず、要件と設計（CI §4, CM §4）に基づき新規に定義する。

### M2 — テキスト会話の縦断 slice（Walking Skeleton 第 1 slice）

- **目的**: Setup → 最初のテキスト会話 → 保存 → 再起動跨ぎの復元、を Host 正本と Client 表示に分けて動かす。round 発行（F-04 解決済み）、Host / Client 境界、`ene-store` の基盤（app.db, Conversation History, 最小 presence）を実コードで固定する。
- **完了状態**: acceptance「Setupと最初の会話」が通る。新規環境で言語と Character `ene` と Provider / model / Credential を選択し、Main LLM への割当を明示して Setup 完了 → テキスト送信 → 同一 timeline で応答 → UI 言語切替で維持、まで。Host 再起動後も保存済み会話が timeline として復元される。
- **次へ進む前の validation**:
  - IPC §26 V-1（connect / authenticate / capability advertise）と V-2（Owner Text → response stream → presentation ack）を実装・テストで成立。
  - CM §12 walkthrough 1（Client input → Companion → inference → response、Learning は後続）の crate hop を確認。
  - `ene-api` が wire-neutral DTO のみで Host 内部 newtype / row / secret を出さないことをテストまたは型で保証。
  - 保存済み round / Conversation History の再起動復元テスト。`cargo test --workspace` / clippy green。
- **実装しなくてよいもの**: Learning / Memory（M3）、Task（M4）、presence 移動・復旧の完全実装（M5）、Body（M7）、Voice / Observation / Schedule / Remote Client / backup、性能測定。
- **注意点**:
  - round 発行 authority は `ene-presentation`。Client 主張（`claimed_generation`）を authority にしない（IPC 原則 2）。`StaleRound` は `Ok` 側 domain outcome で返し、`Err` 側 retry 対象にしない。
  - ID / revision / generation は本文文字列で照合しない（CI §4.6）。時刻は wall-clock + tz を維持し、Schedule tz を黙って置換しない。
  - 保存は短 transaction で atomic。`Transaction` を external に露出しない。処理中・未報告・不明消費はゼロ化しない（PR §6.2）。
  - Provider は OpenAI 単一でよい。fallback・費用 cap・複数 Provider は M16。

### M3 — Experience Summary / Memory 形成（Learning）

- **目的**: `ene-learning` を新設し、Experience Summary を根拠に Companion scope の Memory を形成・更新・訂正できるようにする。由来表示・scope・更新履歴を Text 管理面で確認できるようにする。
- **完了状態**: acceptance「会話からのMemory」のうち 1〜9（message を Summary に圧縮し個別複製しない、Companion scope、由来表示、再起動跨ぎの自然想起、会話による訂正で誤認識の訂正と状況変化を区別、`revision` と根拠の保持、「覚えておいて」の重視と Credential 非保存、過剰保存しない、重複追加しない）が成立する。10〜17（targeted deletion 関係）は M6 で扱う。
- **次へ進む前の validation**:
  - CM §12 walkthrough 1 を Learning までの全 crate hop で成立（`ene-companion` は premise 供給、`ene-learning` が形成判断）。
  - IB V-1 / H-B〜H-E の contract をテストで固定（candidate / decision の分離、scope 変更は `ene-learning` 判断 + `ene-permission` 強制）。
  - 通常の忘却・訂正・失効・置換が保存済み Memory / revision を削除しないことのテスト（requirements「重要度、忘却、订正」）。
  - Credential 値が Summary / Memory / evidence に現れない自動検査（acceptance「利用量と秘密情報」4 の一部）。
  - 保存は PR の Learning group（D2 想定）に従い、derived（embedding 等）を正本にしない。
- **実装しなくてよいもの**: Skill 形成・改善、Relationship、Companion State、Global scope 昇格の完全実装（M13）、targeted deletion（M6）、容量管理の自動 cleanup（既定 OFF。UI は M6）、Observation の形成利用（M11）、グループ会話（M12）。
- **注意点**:
  - Memory / Skill / Relationship / Companion State は互いに第二の正本を作らない。Summary は Raw History の複製にしない。
  - 重要度と scope は別概念。Global へは明示共有または文脈で明確な場合のみ（黙って昇格しない）。
  - 通常の update で過去 revision や根拠を書き換えない。一方で「既存実装の判断」を設計へ逆輸入しない（AGENTS.md の source of truth 規則）。
  - derived 検索 data は Memory の意味内容の唯一の正本にしない。

### M4 — Task Agent 委任 file 作業（Workspace）

- **目的**: `ene-task`（Task・委任・WorkspaceAssoc・TaskContext）と `ene-action`（作用・確定度・file adapter）を新設し、一つの Workspace folder を対象とする file 作業を Task Agent へ委任できるようにする。`ene-permission` の live check・`ene-sandbox` 利用を開始する。
- **完了状態**: acceptance「Workspaceでのfile Task」1〜8 が成立する（既存 file 読取 + 新規 Markdown 作成、Workspace が Task に従属、Task 追跡と Task Agent 委任、Task 中も通常会話・進捗確認・追加指示反映、Cancel は best-effort、正常終了時の変更 file / 保存場所 / 未完了事項の報告、Delete / shell / Network / MCP / 外 path / path traversal の拒否と迂回不能、Workspace 関連付け消失でも file を黙って削除しない）。
- **次へ進む前の validation**:
  - CM §12 walkthrough 2（Task → Task Agent → Action）と 3（Permission → Credential → effect）を成立。
  - IB V-2（Task creation → delegation → steering → result）・V-3（candidate → authorization → external effect → timeout → late result）・V-4（cap 近傍 reservation → usage）をテストで固定。
  - CCT §7（SD-Task steering 競合）・§8（SD-Action 開始前 atomic compare・outcome tracking）・§9（SD-Cap reservation / commit / release）の race walkthrough をテスト化。
  - 作用不明（`Unknown`）の自動再実行が無いこと、crash 後の `Reserved` が release されず「不明」確定されること（F-05 解決済み契約）のテスト。
  - 外部 file への破壊的操作（Delete / shell / Network 等）の拒否と Workspace 外 path の拒否テスト。
- **実装しなくてよいもの**: Schedule（M8）、Computer Use の Client 依存部分（M14）、MCP ツール・Plugin の一般受入（後続）、費用 cap の管理面（M7）、Skill 実行（M13）、Observation タスク（M11）。
- **注意点**:
  - caller ≠ authority：`ene-task` は `ene-permission` / `ene-credential` の具体に直接依存せず、premise 供給で満たす（F-06 是正済み）。live check の caller は `ene-action` / `ene-inference`。
  - 委任・実行に `ExpectedRevision` / `ExpectedGeneration` の CAS を使い、保存済み Allow や copy を authority にしない（Review#3 S-09）。
  - Cancel は best-effort。drop を停止完了とみなさない（CCT §14）。停止できなかった処理・外部作用・未保存作業を報告する。
  - 成果物は外部 file として保存し、ene 専用 library へ複製しない（requirements「Fileと成果物」）。Task 削除で Workspace file を削除しない。

### M5 — Client 不在時の継続・Host 再起動・presence 復旧

- **目的**: `ene-presence` を本格実装し、Client 不在時の Task 継続、Host 再起動後の明示再開待ち（自動再開しない）、Running Companion の presence 復旧、未伝達事項の次 Client 報告を成立させる。
- **完了状態**: acceptance「ClientとHostのlifecycle」1〜5 が成立する（Client を閉じても Host で Task 継続、再接続後に進捗・結果確認、Host 終了→再起動で自動再開せず保存済み進捗と既知の外部作用を示して明示再開を待つ、Running Companion は再起動前 Client へ自動復旧、元 Client が利用不能なら active なしで待ち別 Client へ無条件移動しない、Stopped Companion には適用しない）。
- **次へ進む前の validation**:
  - IPC §26 V-3（Companion move A→B）・V-4（stale A の late input）・V-5（disconnect during Computer Use）・V-6（reconnect with unresolved Action）・V-10（Host restart → reconnect → presence restoration）を成立。
  - CCT §10（SD-Presence の presence generation / CAS による切替）と、通常切断確定 → 利用可能な Host PC Client へ fallback（`DisconnectFallback`）・無ければ `NoActive`、再接続だけで帰属復帰しない（F-01 解決済み）ことをテストで固定。
  - CM の presence 関連 walkthrough（X-A / X-F / X-G / X-H）を crate hop で確認。未伝達の要約報告と提示確認（`ConfirmPresentationObservation`）まで。
  - 明確な自動再開・自動移動がないことのテスト。PR Group G の recovery（`RecoveryWait` は Host restart 復旧に限定）。
- **実装しなくてよいもの**: Remote Client（不必要な pairing・TLS。M14）、Host 側 Client 環境の自動起動、Observation の existence 連動（M11）、Store の自動再開。
- **注意点**:
  - presence の判定材料（transport_class 等）は Host 側で確定し、Client 自己申告を根拠にしない（IPC §10.1・F-01 修正）。
  - 移動は round を安全に区切って行い、二重 presence を禁止（CAS）。未確定 Action を Host 側で自動再実行しない。
  - Stop は disconnect と区別し、Stopped Companion には fallback / 復旧を適用しない。
  - 再起動後の Task 再開は「Owner の明示再開」であり、presence 復旧と独立に扱う。

### M6 — Targeted Deletion・容量管理・Audit

- **目的**: `ene-preservation` を本格実装し、Privacy / Security 目的の targeted deletion（機械検索・削除・残存検証・再形成防止）、容量管理の自動 cleanup（既定 OFF）、Audit の追記順記録を成立させる。
- **完了状態**: acceptance「会話からのMemory」10〜17 と「障害と安全境界」の関連項目が成立する（特定文字列の機械検索・削除・残存検証、復元可能な copy（History / Summary / Memory / revision / evidence / index / embedding / cache / Client 一時 data）の削除または復元不能化、削除完了前に完了表示しない、削除中に再入力・内部生成されても同一消去対象、削除後 consolidation / 再起動でも再形成されない、無関係情報を不必要に削除しない、通常の History 削除は Learning へ cascade しない、容量不足でも自動 cleanup しない、「忘れて」だけでは targeted deletion に昇格しない）。
- **次へ進む前の validation**:
  - CM §12 walkthrough 7（全域 fan-out / participant completion / 残存検証）を成立。
  - IB V-7（削除中の delayed Learning result）をテストで固定。CCT §11（SD-Deletion の durable-before-enforce）を race walkthrough で確認。
  - IPC §26 V-9（Client 不在時の targeted deletion）を Host 同居 Client で成立。
  - PR §8（追跡不能にしないための persistence・過剰設計にしない制限）と Audit（秘密・本文を含まない）のテスト。
- **実装しなくてよいもの**: Backup / Restore / Update（M9）、Reset の完全実装（M9 と合わせる）、Remote Client の削除参加（M14）、容量 cleanup の高度な選択 UI（accepted は既定 OFF 設定のみ）。
- **注意点**:
  - 削除は「durable な範囲決定 → 各 owner の局所消去 → 集約・残存検証 → 完了確定」の順。完了確定は `ene-preservation` の集約のみが行う。
  - 各 participant は全体完了待ちで局所検証を止めない。削除完了表示は残存検証後。
  - 外部に送信・export・backup 済みの copy まで削除したと表示しない（対象は ene 内部 data）。
  - targeted deletion は「一般 editor」ではない。reset は M9 の Backup / Restore と合わせて扱う。

### M7 — 管理面・基本 Body・性能 Gate・両 OS 受け入れ

- **目的**: Text 管理面を整備し、基本 Desktop Body（VRM overlay・idle / 応答中の区別・move / resize / hide）を追加し、Support Matrix の両 OS で acceptance の性能 Gate と baseline を記録する。
- **完了状態**: acceptance「基本Body」1〜4 と「性能Gateとbaseline」が成立する（透明 overlay 表示、移動 / resize / hide と通常操作の継続、idle と応答中の区別、Body 故障時も Text 会話と管理面が使える、idle CPU / Memory / Body FPS / local 操作 1 秒以内の Gate を両 OS で記録）。
- **次へ進む前の validation**:
  - IB V-9（Character revision apply の基盤）を含む、管理面からの操作が各 owner の authority を迂回しないことの walkthrough。
  - IPC §18（高権限操作は Host PC 上の trusted first-party management surface に限定）の契約を管理面の実装で担保。
  - 性能測定期間・手法・試行回数を記録し、両 OS で同一手順の baseline を取得。`cargo test --workspace` / clippy green。
- **実装しなくてよいもの**: Voice（M10）、Observation（M11）、Schedule の UI（M8）、Remote Client（M14）、費用 cap の広域設定（M16）。
- **注意点**:
  - Body は Client 側の表示であり、Host 正本の状態から staging して提示する。表示成功を保存・作用の成立にしない。
  - 性能 Gate は model / 外部 Provider を除く ene 自身に適用。LLM latency と Task 所要時間は baseline 記録に留め hard gate にしない。
  - 両 OS の測定値を根拠なく混在させない。Gate は同一 Release の条件で判定する。

## 6. 後続 milestone（M8 以降）の進め方

M8 以降は acceptance が「後続」とした範囲（requirements の一部）である。各機能は、依存を満たした順に milestone を追加してよい。各 milestone は本Guide の形式（目的 / 完了状態 / validation / 実装しないもの / 注意点）に従い、対応する design artifact を正本とする。

- M8 Schedule: IB H-H、requirements「Schedule」。依存 M4。
  - 完了状態の目安: 作成・変更・停止・削除・即時実行、各回を新 Task として追跡、missed 記録と Run now、timezone 保持。確認が必要な Action は実行せず判断待ち。
  - 注意: Schedule 作成依頼を特別な Permission token にしない。LLM polling しない。
- M9 Backup / Restore / Update・自動起動: CA-BR、IB D-D、PR §9、IPC §26 V-13/V-14。依存 M6, M7。
  - 完了状態の目安: portable full backup（secret・外部 Workspace file 含まない）、暗号化と非暗号の明示、Restore の全置換・旧 live 混入防止・一括有効化、現在の Credential store 維持、失敗時に復元前状態を破壊しない。
  - 注意: Backup は Audit と合わせて保持方針を表示する。Restore 後の自動利用を開始しない。
- M10 Voice: IPC §13.2・V-7、requirements「Voice」。依存 M7。
  - 完了状態の目安: 低遅延（barge-in）→ turn-based → Text の段階的切替、VAD 待受の識別表示と即時 Mute、keyboard 経路。
  - 注意: Voice 障害は Text を妨げない。安全操作（Mute / 停止 / 拒否）は keyboard でも行える。
- M11 Observation / Observer: IB X-D/X-E/X-F・H-F、IPC §14・V-8、requirements「Observation」。依存 M4, M5, M7。
  - 完了状態の目安: Client 単位の Capture / 候補検知 / routing（三層分離）、Observer 専用 Provider assignment、Pause / OFF、観測頻度、fullscreen 休止。
  - 注意: Raw は保存しない。routing 文脈は各 source owner が範囲限定で供給し、`ene-core` は中継のみ（F-07 解決済み）。
- M12 グループ会話・Companion 間交流・自発性: requirements「グループ会話」「自発的な発話と行動」「Companion間交流の記録」。依存 M3, M5。
  - 注意: 交流 History と Learning を別 lifecycle にし、一方・両方の削除を理由に History を削除しない。
- M13 Global scope・Skill 形成改善・Companion State・Relationship: IB H-E、requirements「Learningと成長」。依存 M3。
  - 完了状態の目安: Global scope 条件（明示共有・文脈から明確）、Skill の revision / 未検証・成功・失敗の区別、Companion State の一時的 / 持続的傾向の区別、Relationship の主体別状態。
  - 注意: Global 昇格を黙って行わない。Relationship で Permission / Rule / Capability / 安全境界を変更しない。
- M14 Remote Client: IPC §9〜§12・§18、CA-CPT、PR Group E/G/K、requirements「Remote Client」。依存 M5, M7, M9。
  - 完了状態の目安: device pairing（Host PC 上の trusted first-party management surface で最終確認）、LAN/VPN、TLS、device ごとの失効、presence 移動、Client 一時 data の非永続。
  - 注意: 失効が Restore で巻き戻らないこと（F-02 解決済み：Host device-auth は E 分類・backup 除外）。通常切断 → Host PC Client への fallback（F-01）。
- M15 Character 編集・Package 配布: IB C-A〜C-D、requirements「Character Package」。依存 M1（ene-character 拡張）。
  - 注意: export 前に内容と権利上の注意を確認。適用は Companion の Experience 由来状態を黙って上書きしない。
- M16 複数 Provider・fallback・費用 cap 完成: IB K-D/K-E/K-F、requirements「割当と同意」「Fallbackと費用」。依存 M6, M7。
  - 完了状態の目安: Provider ごと・全体の cap、承認済み fallback 順序、能力不足の事前提示、Provider 報告値 / 推定値 / 不明の区別表示。
  - 注意: 登録だけで利用開始しない（明示割当が必要）。未承認 Cloud への自動移行をしない。

## 7. 自律判断と Escalation（人間へ委ねる条件）

### 7.1 自律判断してよい範囲の要約

§2.3 のとおり、以下は人間の確認なしに実装者が決定してよい。

- Rust type 細部 / method 名 / module 内部構造 / library 利用方法
- SQL column / index / migration の具体形（table group・owner 注記に従う範囲）
- retry / timeout / backoff 値、費用予約量の算定式、観測頻度・scheduling algorithm、prompt 組立、cache 実装
- concurrency mechanism の選択（CAS / owner-local mailbox / 短 transaction 内 atomic read+insert 等）
- sync / async 粒度、test 実装、performance 上の局所判断

### 7.2 人間へ escalate すべき条件（Issue として扱う）

以下のいずれかに該当する問題を実装中に発見した場合、実装で勝手に意味を決めず **Issue として扱う**。

- Requirements / Design の**固定前提の変更**が必要になる問題（semantic owner の変更、identity / revision / generation の意味変更、serialization domain の統合、caller ≠ authority の崩れ、secret 非返却の緩和、保存分類の変更、IPC の Host-local / remote-capable 境界の変更等）
- 既存 Requirements / Architecture を変更しなければ解決できない問題（新たな製品挙動の追加、安全境界の変更、Privacy / Security contract の変更）
- 下位 artifact ↔ 上位 artifact の矛盾（design README precedence rule 4）
- レビュー型の欠落（Review#3 の finding 種別 (c) security property の担い手欠落、(d) trust boundary の拡張、(e) bootstrap gap に相当する新規発見）

### 7.3 Issue の記録方法

1. 該当する design artifact の Issue 節（CM §15 / IB §19 / IPC §29 / PR §14 / CCT §21 / CI §12 等）へ追記する。各 artifact の指示に従い、「変更が必要に見えた場合は設計で吸収せず Issue として報告する」を守る。
2. 既存 Requirements / Design を自分で変更しない（Issue として記録し、人間の判断を待つ）。
3. 解決までブロックされる作業は、可能な範囲で別作業へ切り替える（同一 milestone 内の独立部分）。blocker として §8 の進捗管理に記録する。
4. 本Guide（docs/implementation/README.md）への修正が必要になったときは、Guide 自体は更新してよい（Guide は navigation であり正本ではない）。

## 8. 進捗管理（Progress tracking）

長時間の自律実装で進捗を失わないため、Repository 上に**単一の進捗記録**を置く。巨大な task management system は作らない。

### 8.1 記録場所

`docs/implementation/PROGRESS.md` を新規作成し、次の 4 項目だけを維持する。

- **現在の milestone**: 作業中の milestone（例: M2 テキスト会話の縦断 slice）
- **完了した milestone**: 完了順の一覧（例: M0, M1）
- **次に進む領域**: 次の subsys / crate / 検証項目（例: ene-presentation round、IPC V-2 テスト）
- **未解決 blocker**: Issue 番号と理由（§7.3 の記録と対応）

### 8.2 更新タイミング

- milestone 開始時: 現在の milestone を更新。
- milestone 完了時: 完了した milestone へ移動し、次に進む領域を記載。
- Issue / blocker 発生時: 未解決 blocker へ追記。
- 各作業セッションの終了時（/goal が長時間動く場合）: 現状を反映。

### 8.3 推奨テンプレート

```markdown
# 実装進捗

## 現在の milestone

- M2 テキスト会話の縦断 slice（作業中）

## 完了した milestone

- M0 退避・workspace 再構成
- M1 基盤 crate の新規構築

## 次に進む領域

- ene-presentation round 発行と IPC V-2 の統合テスト

## 未解決 blocker

- なし
```

進捗は milestone の粒度で更新し、task ごとの詳細は Git branch / PR / commit message に残す。進捗記録の更新そのものが実装作業を妨げない範囲に留める。

## 9. Continuous validation の進め方

実装は最後に一度だけ検証するのではなく、milestone ごとに build / test / lint / integration 等の適切な validation を行う。Architecture 上重要な境界については、実装が進むにつれて実コードで成立していることを検証する。

### 9.1 milestone 完了時の共通ゲート

- `cargo fmt --all -- --check`
- `cargo check -p <pkg>` / `cargo test -p <pkg>`（focused）
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- 該当する design walkthrough（CM §12 / IB §17 / IPC §26 / CCT §18 / PR §12）の消化。
- acceptance の該当シナリオの再現確認（両 OS のうち実装環境で可能なもの。両 OS 総合は M7 で実施）。

### 9.2 Architecture 境界の検証対象（milestone 別）

| 境界 | M1 | M2 | M3 | M4 | M5 | M6 | M7 |
|---|---|---|---|---|---|---|---|
| caller ≠ authority（IB §2） | 型で担保 | round 受理で確認 | task で確認 | action 認可で確認 | presence で確認 | deletion 集約で確認 | 管理面で確認 |
| compare-before-commit（CCT §5） | — | round / History 保存 | Memory 更新 | Task / Action / Cap | presence CAS | deletion token | restore 前置き |
| secret 非返却（DR-05 / IB §11） | credential split で開始 | 自動検査 | Summary / Memory 検査 | Tool arg 検査 | 移動時検査 | deletion 検査 | Debug capture 検査 |
| 保存分類（PR §3） | — | app.db 導入 | derived 分離 | Group D/F | Group G | Group J | 容量測定 |
| Host / Client 境界（CM §11 / IPC） | ene-api narrow | IPC V-2 | — | — | IPC V-3〜V-6 | IPC V-9 | IPC §18 |
| SD 分離（CCT §4） | — | SD-Round | SD-Learn | SD-Task / Action / Cap | SD-Presence | SD-Deletion | — |

### 9.3 テストの置き場所

各 owner crate の原則テストは crate 内に置く。境界をまたぐ統合テストは、呼び出し側 crate（caller）に置き、依存 crate の public interface だけを使う。耐久・復元・削除・restore の永続テストは `ene-store` 側に置く。

## 10. 完了条件と出荷判断（Exit criteria）

milestone ごとの完了は §5 の「完了状態」で判定する。ただし、**milestone の完了は受け入れ条件（acceptance）の完了を意味しない**。acceptance の完了は M7 終了時に §5 M7 の完了状態と次を満たしたときだけ宣言する。

- Acceptance の完了条件（両 OS・両言語で代表シナリオが再現可能な手順により完了）を満たす。
- 最低性能 Gate を満たし、性能 baseline と既知の制約が記録されている。
- 失敗した検証が、再現条件・data への影響・回避策・次の判断とともに記録されている。
- 要件を満たすための未実装範囲が、この milestone の完了によって完了扱いになっていない（後続 milestone へ明示される）。

出荷・公開・Support Matrix の更新・Provider 既定・費用・法律上の判断は、本Guide の範囲外であり人間の判断に委ねる。

## 11. この Guide の使い方（/goal 向け）

1. 作業開始時: `docs/implementation/PROGRESS.md` を読み、現在の milestone と次に進む領域を確認する。
2. milestone 内の作業: §5 の該当 milestone と、参照する design artifact（CM / IB / PR / CCT / CI / IPC）を読み、§2 の規律に従って実装・検証を回す。
3. 判断: §7.1 の範囲は自律判断。§7.2 の条件に該当する問題は Issue 化し、独断で進めない。
4. milestone 完了時: §9 の共通ゲートと §5 の validation を実行し、PROGRESS.md を更新して次の milestone に進む。
5. 本Guide 自体は必要に応じて更新してよい（milestone の追加・順序調整・validation の具体化）。正本である Requirements / Design は Issue を経てのみ変更される。

## 12. 付録: 主要 artifact への参照

- [Requirements README](../requirements/README.md)（製品挙動の正本）
- [acceptance.md](../requirements/acceptance.md)（最初の milestone 範囲・代表シナリオ・完了条件）
- [Design README](../design/README.md)（優先順位・precedence rule）
- [Crate / Module 分解（CM）](../design/concrete/crate-module-decomposition.md)（移行段階 §13・walkthrough §12）
- [Interface Boundaries（IB）](../design/concrete/interface-boundaries.md)（V-1〜V-9）
- [Persistence / Recovery（PR）](../design/concrete/persistence-recovery.md)（保存分類・group 表）
- [Concurrency Control（CCT）](../design/concrete/concurrency-control.md)（SD・compare-before-commit）
- [Correspondence / Identity（CI）](../design/concrete/correspondence-identity.md)（ID / revision / generation / correlation）
- [Host ↔ Client IPC](../design/concrete/host-client-ipc.md)（V-1〜V-14・§18 管理面）
- [Architecture Review #3](../design/reviews/architecture-review-3.md)（統合済み判断の記録）

