**ene-config の実装・外部クレート置換調査（2026-09-22）**

`ene-config` の読込処理は外部クレートに置き換えられる。ただし、現在の2項目のために汎用設定ライブラリを追加する利点は小さい。推奨は、起動時のデータディレクトリ、CLIが送る本文の言語、GUIの表示言語を分離すること。手編集するJSON設定を廃止するなら、既存の `clap` と `directories` で共通ローダーをなくせる。JSONを残す場合も、現在の `serde_json` による小さな読込処理を維持する方が簡単である。

この文書は調査結果と設計変更の提案であり、実装契約を変更しない。調査対象は `main` の `663b2075` と調査時の作業ツリー。開始時から変更されていた desktop の `session.rs`、`shell.rs`、`ui/mod.rs`、`ui/runtime.rs` も現状として参照した。本調査では製品コード、Cargo manifests、リポジトリの `Cargo.lock` を変更していない。

実装は3ファイル、合計244行。テスト部分を除くと、コメント・空行込みで127行に収まる。`load_with_env` はシグネチャを含めて19行で、独自のJSONパーサーや汎用マージエンジンは実装していない。

| 部分 | 現在の処理 | 外部への委譲 |
|---|---|---|
| `Config` | `language: String` と `data_dir: Option<PathBuf>`。既定値は `ja` と `None` | Serdeによる型への変換 |
| `Config::load` | 既定値 → 指定されたJSON → `ENE_LANGUAGE` / `ENE_DATA_DIR` → 検証 | JSON解析は `serde_json` |
| `Config::validate` | 言語が空または空白だけならエラー | Ene固有の入力規則 |
| `default_data_dir` | `ProjectDirs::from("dev", "ene", "ene")` の `data_dir()` | `directories` 6.0.0 |
| `resolve_data_dir` | 明示パスを優先し、なければOSの既定ディレクトリ | 選択規則のみ自前 |
| `ConfigError` | 空言語、ファイル読込、JSON解析のエラー | `thiserror` |

実装根拠: [typed.rs](/home/pexisgle/dev/Ene/crates/ene-config/src/typed.rs:17)、[paths.rs](/home/pexisgle/dev/Ene/crates/ene-config/src/paths.rs:16)、[Cargo.toml](/home/pexisgle/dev/Ene/crates/ene-config/Cargo.toml:9)。

ファイルの自動探索、保存、変更監視、再読込、秘密情報の保存はない。`load(None)` はOSの設定ディレクトリを探さない。指定ファイルが存在しない場合は既定値へ戻るが、それ以外の読込失敗は返す。JSONの構文・型の検査は環境変数による上書きより先に行う。したがって `{"language":42}` は、`ENE_LANGUAGE=en` があっても失敗する。一方、空文字列はJSONの型検査を通るため、環境変数で置き換えた後に検証される。

パス解決にはディレクトリ作成の副作用がない。Host、CLI、GUIが同じアプリ識別子と既定パスを使う点が、この共通クレートの主な価値である。ファイル数の削減だけを目的に同じ式を各アプリへ複製すると、接続先の食い違いを起こす余地が増える。

`directories::ProjectDirs` はLinuxではXDGのデータ領域、WindowsではRoaming AppDataを `data_dir()` として返す。現在の指定なら、通常はLinuxの `~/.local/share/ene`、Windowsの `%APPDATA%\ene\ene\data` になる。Windowsのローカル専用領域を使う `data_local_dir()` は別APIなので、切替は保存場所の設計変更として扱う。[directories公式API](https://docs.rs/directories/6.0.0/directories/struct.ProjectDirs.html#method.data_dir)

実際の依存範囲は設計図より狭い。`cargo tree -i ene-config --locked -e normal` とソース参照を突き合わせると、直接依存は次の4パッケージだけだった。

| 利用者 | 実際の用途 | 変更の影響 |
|---|---|---|
| `apps/ene-core` | `--config` を読み、データディレクトリを解決。言語値は使用しない | 起動・管理コマンドの設定読込を変更 |
| `apps/ene-ctl` | `--config`、データディレクトリ、送信本文の言語タグ | CLIの起動引数と `TextLangWire` の生成を変更 |
| `apps/ene-desktop` | `Config::load(None)` からデータディレクトリだけを使用 | GUI/launcherの起動入口を変更 |
| `crates/ene-client` | Cargo依存宣言のみ。Rustソースに利用箇所なし | 依存宣言を削除可能と判断 |

根拠: [coreの起動](/home/pexisgle/dev/Ene/apps/ene-core/src/main.rs:280)、[ctlの読込](/home/pexisgle/dev/Ene/apps/ene-ctl/src/main.rs:571)、[desktopの読込](/home/pexisgle/dev/Ene/apps/ene-desktop/src/shell.rs:217)、[clientのmanifest](/home/pexisgle/dev/Ene/crates/ene-client/Cargo.toml:10)。ドメインクレートや `ene-body` に現在の直接依存はない。設計の依存許可表に名前が載っていることと、実装で依存していることは区別する。

HostとGUIは、相手のプロセスを起動するときに解決済みのパスを `ENE_DATA_DIR` で渡している。再設計でもこの受渡しを揃える必要がある。設定ライブラリを替えるだけでは、接続先ディレクトリの一致は保証されない。[GUI→Host](/home/pexisgle/dev/Ene/apps/ene-desktop/src/host_launch.rs:87)、[Host→GUI](/home/pexisgle/dev/Ene/apps/ene-core/src/host_control.rs:948)

調査で最も問題になったのは `language` の意味の混在だった。公開コメントは「UI locale」としているが、CLIでは送信本文の `TextLangWire` に使われる。このwire型は表示・ルーティング用の不透明な言語タグであり、GUIが提供する日本語/英語の選択肢と同じ制約とは限らない。単に共通の `Ja / En` enumへ置き換える案は、この差を無視してしまう。[本文への設定](/home/pexisgle/dev/Ene/apps/ene-ctl/src/cmds.rs:261)、[wire型の契約](/home/pexisgle/dev/Ene/crates/ene-api/src/v1/refs.rs:84)

GUIは `Config.language` を使わず、`desktop-locale` から `Locale` を読み込む。保存値がなければ日本語になる。そのため、読んだコードの経路では `ENE_LANGUAGE=en` だけでGUIを英語にできず、`ENE_LANGUAGE=""` はGUIが必要とするパスを得る前に起動エラーになる。Hostも使わない言語の検証に巻き込まれる。[GUI側の読込](/home/pexisgle/dev/Ene/apps/ene-desktop/src/ui/runtime.rs:76)、[ロケールの保存先](/home/pexisgle/dev/Ene/apps/ene-desktop/src/ui/runtime.rs:1450)

UI言語の選択・切替は製品の受け入れ条件に含まれる。一般設定の正本は上位設計で「入出力・提示」に置かれており、`ene-config` にrepositoryや状態判断を持たせることは禁止されている。`desktop-locale` を恒久的な正本とするなら、この所有権との整合を先に決める必要がある。接続前のセットアップで必要な表示言語と、接続後の一般設定の正本をどう扱うかが設計上の論点になる。[受け入れ条件](/home/pexisgle/dev/Ene/docs/requirements/acceptance.md:48)、[一般設定の所有者](/home/pexisgle/dev/Ene/docs/design/architecture/state-ownership.md:238)、[configの責務](/home/pexisgle/dev/Ene/docs/design/concrete/crate-module-decomposition.md:100)

GUI側には、言語設定の保存失敗をすべて無視する処理もある。画面上では切り替わっても、保存に失敗すると再起動後に戻る。これは `ene-config` の直接の不具合ではないが、設定処理を整理する際の対象になる。設定の読み取りと書き込みを一つの汎用ライブラリへまとめても、保存の成否をユーザーへ伝える責任は残る。[persist_locale](/home/pexisgle/dev/Ene/apps/ene-desktop/src/ui/runtime.rs:1457)

起動設定には、ほかにも見直す余地がある。

- 明示した `--config` が存在しなくても成功する。タイプミスで意図しない既定データ領域を選ぶ可能性がある。明示指定の失敗を報告し、自動探索する任意ファイルの不在だけを許容する設計が分かりやすい。
- `ENE_DATA_DIR` を `std::env::var` で読むため、非Unicodeのパスは無視される。`PathBuf` に渡す値は `var_os` で取得し、意図せず既定ディレクトリへ切り替わらないようにする方がよい。
- 空の `data_dir` と相対パスを受け入れる。空値は拒否し、相対パスをどのディレクトリ基準で扱うかは起動入口で確定する案を推奨する。解決時にディレクトリを作成する必要はない。
- 未知のJSONキーは無視され、`{"langauge":"en"}` のような誤記は日本語の既定値になる。JSONを残すなら、未知キーをエラーにするかを決める。
- `ConfigError::Read` は対象パスを保持しない。複数ファイルを導入する場合は特に診断しにくい。
- [paths.rsのコメント](/home/pexisgle/dev/Ene/crates/ene-config/src/paths.rs:12)に「将来の変更時には既存ディレクトリを移行する」とあるが、[AGENTS.md](/home/pexisgle/dev/Ene/AGENTS.md:5)は互換・移行層を対象外としている。このコメントを移行コード追加の根拠にしない。

これらは外部クレートで自動的に解消される問題ではない。どの入力を許可し、どの失敗を報告するかはEne側で決める必要がある。以下の比較でも、現状との差をすべて不具合とは扱っていない。後方互換性は不要なので、必要な挙動は要件と設計から選び直せる。

外部候補は公式API、公開ソース、crates.ioの配布情報で確認した。バージョンは2026-09-22の確認値である。

| 候補 | 置き換えられる範囲 | 今回の判断 |
|---|---|---|
| `directories` 6.0.0 | OS標準ディレクトリの解決 | 採用済み。継続でよい |
| 既存の `clap` 4.6系 | CLI引数、環境変数、既定値、型付き引数の解析 | JSONを廃止する再設計なら第一候補。`env` featureの追加で対応可能 |
| `config` 0.15.26 | ファイル・環境変数・既定値のマージ、Serdeへの変換 | 多層設定が実際に必要になった場合の候補。現在の2項目には利点が小さい |
| `figment` 0.10.19 | 複数providerのマージ、設定値の出所追跡 | 出所付き診断が必要な場合の候補。環境変数とファイル探索の既定挙動に注意 |
| `confique` 0.4.0 | 型に付けた属性による既定値・環境変数・検証の宣言 | 宣言の集約には向く。JSON5採用と生成コードの依存を受け入れる必要がある |
| `confy` 2.0.0 | OSの設定パス選択、設定の読込・保存 | 今回の起動時ローダーの置換には不向き |

`clap::Arg::env` は環境変数を引数の入力元として扱える。現workspaceの `clap` は `default-features = false` で、`env` はまだ有効にしていない。[clap公式API](https://docs.rs/clap/latest/clap/struct.Arg.html#method.env)、[workspace依存設定](/home/pexisgle/dev/Ene/Cargo.toml:14)

`config` を導入するなら、現用途では `default-features = false, features = ["json"]` で足りる。標準featureには複数のファイル形式などが含まれるため、そのまま有効にする理由はない。今回の限定構成でも `config`、`pathdiff`、`winnow` が `ene-config` の現在の依存木に加わる。これはクレート単位の比較であり、workspace全体に新規追加されるパッケージ数やビルド時間の測定ではない。[configのfeatures](https://docs.rs/crate/config/0.15.26/features)

`confy::load` / `load_path` は設定ファイルがないとディレクトリと初期設定を書き込む。`load_or_else` も、欠落や内容の不正から既定値を作って保存する。読み取り時に無関係な永続変更をしないという本リポジトリの規約には、そのままでは合わない。環境変数の重ね合わせも別途必要になるため、今回は採用を勧めない。[confyの公開実装](https://docs.rs/confy/2.0.0/src/confy/lib.rs.html#315-447)

比較実験は本番の設定・データ領域を使わず、別のCargoプロジェクトと一時ファイルで行った。現実装、`config`、`figment`、`confique` に同じ入力を与え、最後に同じ空言語検証を実行した。21ケース×4実装と、親ディレクトリ探索の追加1ケースを確認した。欠落ファイルの読込でディレクトリが作られないことも4実装で確認済み。`confy` は文書とソースによる比較のみで、実行比較には含めていない。

比較時の構成は `config` がJSONのみ・任意ファイル・明示した2環境変数、`figment` がJSONとenv・`Json::file`・`Env::only`、`confique` がJSON5のみ・`builder().env().file(...)`。`config` の環境入力は `Environment::source` で2キーに絞り、`std::env::var` に成功した値だけを渡した。したがって非Unicode環境変数を無視する結果は、このアダプターの挙動である。[Environment::source](https://docs.rs/config/latest/config/struct.Environment.html#method.source)

| 入力・条件 | 現実装 | config | figment | confique |
|---|---|---|---|---|
| 既定値・部分JSON・環境変数による上書き | 正常 | 正常 | 正常 | 正常 |
| 空の `ENE_LANGUAGE` | エラー | エラー | エラー | エラー |
| 壊れたJSON構文＋正常な環境変数 | エラー | エラー | エラー | エラー |
| `language: 42`＋`ENE_LANGUAGE=en` | エラー | `en` | `en` | エラー |
| `language: 42` のみ | エラー | 文字列 `"42"` | エラー | エラー |
| `language: null` のみ | エラー | エラー | エラー | 既定の `ja` |
| `language` キーの重複 | エラー | 後の値 | 後の値 | エラー |
| 未知キー `langauge` | 無視 | 無視 | 無視 | 無視 |
| JSON5の引用符・末尾カンマ | エラー | エラー | エラー | 受理 |
| `ENE_DATA_DIR=123` | パス `123` | パス `123` | 型エラー | パス `123` |
| 非Unicodeの `ENE_DATA_DIR` | 無視 | 無視※ | 置換文字を含むパス | エラー |
| ディレクトリを設定ファイルに指定 | エラー | 既定値 | 既定値 | エラー |
| 読込権限のない設定ファイル | エラー | 既定値 | エラー | エラー |
| JSON内の非UTF-8パス | エラー | 置換文字を含むパス | エラー | エラー |
| 相対指定のファイルが親ディレクトリだけにある | 既定値 | 既定値 | 親のファイルを読む | 既定値 |

※上記の環境入力アダプターによる。エラー文面の一致ではなく、受理・拒否と解決値を比較した。

`config` の `required(false)` は、調べた0.15.26の実装では読込元の解決失敗を空の設定として扱う。JSON解析に入った後のエラーとは扱いが違う。実験でも権限エラーが既定値に変わった。明示した設定ファイルには `required(true)` を使うなど、失敗方針を決めてから導入する必要がある。[配布ソースのFile実装](/home/pexisgle/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/config-0.15.26/src/file/mod.rs:128)

`figment` の環境変数は文字列を構造化された値として解析するため、数値に見える相対パスがそのまま文字列として残るとは限らない。また `Json::file` は相対パスを親ディレクトリまで探索する。これは `file_exact` で抑止できる。上の表は単純置換時の差であり、カスタマイズ不能という意味ではない。[Envの解析規則](https://docs.rs/figment/0.10.19/figment/providers/struct.Env.html)、[fileとfile_exact](https://docs.rs/figment/0.10.19/figment/providers/struct.Data.html#method.file)

`confique` はフィールドごとに環境変数名を指定でき、全 `ENE_*` を収集せずに済む。なお、フィールド単位の検証は各入力層で、構造体単位の検証はマージ後に実行される。今回の実験は現実装に合わせてマージ後に空言語を検証した。[confiqueのderive API](https://docs.rs/confique/0.4.0/confique/derive.Config.html)

環境変数の採用範囲は明示しておくべきである。Eneには認証情報用の `ENE_OPENAI_API_KEY` なども存在する。汎用ライブラリで単に全 `ENE_*` を収集すると、最終の構造体で未知フィールドが捨てられても、途中の設定ツリーに秘密値を取り込む。`figment` なら `only`、`config` なら明示したsource、`confique` や `clap` ならフィールド別の環境変数指定で区別できる。秘密を扱うクレートや権限・費用上限の所有者を、汎用設定へ統合する理由にはならない。

私が進めるなら、次の順で変更する。

1. 起動時に決めるパス、本文の言語タグ、表示言語の所有者を設計文書で分ける。表示言語の正本と接続前の仮設定もここで整理する。`ene-config` の公開責務を変更する場合は [crate-module-decomposition](/home/pexisgle/dev/Ene/docs/design/concrete/crate-module-decomposition.md:100) を更新する。
2. `ene-client` の未使用依存を削除する。Hostの共通設定から言語検証を外し、CLIの本文言語は `ene-ctl` で扱う。
3. 起動JSONを廃止する案を採る。要件・受け入れ条件に手編集JSONの維持要求は見つからず、現在GUIはそのファイルを読んでいない。core/ctlのCLI入力は既存 `clap` に寄せ、環境変数も型付きで受け取る。GUIが読むパスは `var_os` を使う。パス選択の共通関数と `directories` は小さく残す。
4. JSONを残す必要がある場合は、3の代わりに `serde_json` の薄いローダーを残す。汎用ライブラリ導入は、複数ファイル・設定階層・出所付き診断といった具体的な要求が生じた時点で判断する。
5. GUIの設定保存を、その所有者と整合する経路へ直す。保存失敗を扱い、保存・再起動後の復元を検証する。設定Resetやbackup/restoreの実装時にも同じ所有者を使う。

この案では、`ene-config` を丸ごと削除する必要はない。`Config` と汎用ローダーをなくして、共通のパス方針だけを残す意義がある。`ene-paths` への改名は任意で、削減効果を得るための前提ではない。別の巨大な「設定管理クレート」を新設することも勧めない。

| 変更対象 | 推奨案での作業 |
|---|---|
| `ene-config` | パス解決を `Config` 全体から独立させ、空パス・解決不能の失敗を扱う。起動JSON廃止時は `typed` を削除 |
| `ene-core` | 各コマンドで繰り返す設定読込を入口へまとめ、言語に依存せずHostのパスを確定 |
| `ene-ctl` | 接続先パスと本文言語をCLI側で受け取り、既存のwire DTOに渡す |
| `ene-desktop` | `Config::load(None)` をパス入力へ置換。表示言語の読込・保存は設計に沿って整理 |
| `ene-client` | 未使用のmanifest依存を削除。接続APIは引き続き呼出側からパスを受け取る |
| `ene-presentation` / `ene-store` / `ene-api` | GUI一般設定をHostの正本へ接続する場合の追加影響。起動ローダーの変更だけなら不要 |
| ドメインクレート / `ene-body` | 起動ローダー整理だけなら直接の変更は不要 |

実装を進める際は、Host/GUI/CLIで接続先が一致すること、パス・設定の読込だけで永続変更が起きないこと、GUI言語の切替と再起動で履歴が保たれることを確認する。非Unicodeパス、空値、明示ファイルの読込失敗も選んだ仕様に合わせて検証する。過去の挙動を維持するためのshimや移行層は追加しない。

検証はLinux・Rust 1.98.1で実行した。`nix-shell --run 'cargo test -p ene-config --locked'` は6件すべて成功し、`cargo clippy -p ene-config --all-targets --locked -- -D warnings` も成功した。依存範囲は `cargo tree` と全文のシンボル検索で確認した。候補3クレートは別プロジェクトでコンパイル・実行したため、本リポジトリの依存解決には混ざっていない。

実験コード・固定した依存・全結果は [再現資料アーカイブ](/home/pexisgle/.codex/artifacts/ene-config-2026-09-22/ene-config-reproduction.tar.gz) に保存した。現実装のソースも調査時点のものを同梱している。展開後、Linuxの一般ユーザーで `cargo build --locked`、`python3 probe.py` を実行すると比較を再現できる。調査時の実験は [main.rs](/tmp/ene-config-study-IUxPBG/main.rs) と [probe.py](/tmp/ene-config-study-IUxPBG/probe.py) からも確認できる。

Windowsのパスは公式資料と実装に基づく確認で、Windows実行試験は行っていない。GUIについても今回はソースの呼出経路を確認したもので、起動再現試験は行っていない。全workspaceのビルド・受け入れ試験、候補の性能・バイナリサイズの比較は未実施。判断は実装規模、責務、観察した入力挙動に基づく。
