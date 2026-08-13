# Windows AppContainer サンドボックス強化 実装引き継ぎ

> このドキュメントは Linux 開発環境から Windows 実機での作業へ引き継ぐための
> 自己完結メモです。チャット履歴なしで作業を再開できることを目的とします。

## 1. やること（要約）

`crates/ene-sandbox/src/windows.rs` のモジュール冒頭に書かれている
「Restricted-token / AppContainer hardening ... is the documented next step:
it requires a custom `CreateProcessAsUserW` spawn path and is not yet wired
here」を実装する。上位要件は `plans/sandbox-and-downloads.md` §3.3：

- AppContainer + Low Integrity で低権限の分離コンテキストで実行
- 明示的な ACL（ユーザーファイル・資格情報領域へのアクセス禁止）
- ネットワーク Capability なし（直接のネットワークアクセス禁止）
- Job Object（プロセス・メモリ・CPU 制限）
- 制限付きトークン

現在の Windows 実装は Job Object（CREATE_SUSPENDED → ジョブ割当 →
resume、kill-on-close）のみ。AppContainer は未配線。

## 2. 技術上の制約（調査済み・設計判断が必要な点）

### 2.1 カスタム spawn が必要

`std::process::Command` では AppContainer を指定して起動できない。
`CreateProcessW` + `STARTUPINFOEXW` +
`PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES`（値は
`SECURITY_CAPABILITIES { AppContainerSid, Capabilities: 空配列, ... }`）で
起動する。Capability を空にすることで「ネットワーク Capability なし」になる。

`std::process::Child` は stable では生ハンドルから構築できないため、
`ene_sandbox::windows` に独自 `SandboxedChild` を実装する：

- `kill()` → `TerminateProcess`
- `wait()` / `try_wait()` → `WaitForSingleObject`
- `id()` → PROCESS_INFORMATION.dwProcessId
- 終了コード → `std::os::windows::process::ExitStatusExt::from_raw`

ホスト（`crates/ene-plugin-host/src/manager.rs`）は Windows で
`SupervisedPlugin.child` の型と spawn 呼び出しをこの型に切り替える。

### 2.2 コマンドライン・環境・標準ハンドル

`CreateProcessW` へ渡すデータは `std::process::Command` から取り出す：

- コマンドライン: `get_program()` + `get_args()` を
  `CommandLineToArgvW` 互換ルールでクォートして結合
- 環境ブロック: `get_envs()` から UTF-16 の `"K=V\0"` 連結 + 末尾 `\0\0`
- カレントディレクトリ: `get_current_dir()`
- 標準入出力: `STARTF_USESTDHANDLES` + `GetStdHandle` の3つ
  （`bInheritHandles = FALSE` でも STARTF_USESTDHANDLES 指定時は引き継がれる。
  現状 std spawn がハンドル継承している挙動を維持するため）

### 2.3 名前付きパイプの ACL（必須）

AppContainer プロセスは、サーバー側パイプの DACL に自身の AppContainer SID
への ALLOW ACE が無いと接続できない。tokio の
`named_pipe::ServerOptions` には DACL API がないため、
`crates/ene-plugin-proto/src/transport.rs` の `IpcListener::bind`（Windows）
を `CreateNamedPipeW` + `SECURITY_ATTRIBUTES`（DACL 付き）に置き換え、
`tokio::net::windows::named_pipe::NamedPipeServer::from_raw_handle` で
ラップする。`accept()` が作る次インスタンスにも同じ DACL を適用する。
インスタンス数は `PIPE_UNLIMITED_INSTANCES`（tokio のデフォルトと同じ）。

適用対象（全てのプラグイン向けリスナー）:

- `manager.rs` のプラグイン用パイプ（`ene-plugin-<name>`）
- `ipc_stt.rs` / `ipc_vad.rs` / `ipc_tts.rs` のプロバイダー用パイプ
- `ene-store/src/host_service.rs` の共有ホストサービスパイプ
  （`ene-host-service`）— 登録済み全プラグインのコンテナ SID を許可

SID は `DeriveAppContainerSidFromAppContainerName(プロファイル名)` で
プロファイル作成なしに導出できる（パイプ ACL 用）。

### 2.4 ループバック免除（要・設計判断）

AppContainer は同一 AppContainer 内のプロセス間でも TCP ループバックを
デフォルトで遮断する。このアプリは llama-server / voicevox / whisper を
`127.0.0.1` のサイドカーとして起動する設計のため、各プラグイン
AppContainer へのループバック免除が必須。

候補:

1. `NetworkIsolationSetAppContainerConfig`（firewallapi.dll）で
   コンテナ SID を免除リストに追加。ただし `CheckNetIsolation` 相当の操作は
   **管理者権限が必要**なため、ホストの権限要件が変わる（要判断）。
2. AppContainer を断念し、`CreateRestrictedToken` + 低整合性
   （`SetTokenInformation(TokenIntegrityLevel)`）＋ Job Object のみにする。
   この場合ネットワークは遮断されず、§3.3 の「ネットワーク Capability なし」
   とは矛盾するが、サイドカーは動く（要判断）。

**この判断（管理者権限を前提にするか／代替構成にするか）が未決定。**

## 3. 変更ファイル一覧（推奨順序）

1. `crates/ene-sandbox/src/spec.rs`
   - `SandboxSpec` に `name: String`（`#[serde(default)]`）を追加
     （AppContainer プロファイル名。ホストがプラグイン名を入れる）
2. `crates/ene-sandbox/src/windows.rs`
   - `spawn(command: &std::process::Command, spec: Option<&SandboxSpec>)`
     を実装（AppContainer + Job Object + 上記 2.1〜2.2）
   - `SandboxedChild` 実装
   - AppContainer プロファイル生成
     （`CreateAppContainerProfile` は既存なら開く）、`Drop` で
     `DeleteAppContainerProfile`（ベストエフォート）+ `FreeSid`
   - 既存 `prepare_command` / `attach` / `JobGuard` は spawn 内部化または削除
3. `crates/ene-plugin-host/src/manager.rs`
   - Windows の spawn 箇所（`apply_sandbox_to_command` 付近、restart 含む
     4〜5 箇所）を `windows::spawn` に置換
   - `SupervisedPlugin.child` を Windows で `SandboxedChild` に
   - `build_plugin_sandbox` で `name` を設定
4. `crates/ene-plugin-proto/src/transport.rs`
   - `IpcListener::bind` にコンテナ名（複数可）を受け取る Windows 用パスを追加
5. `crates/ene-store/src/host_service.rs` / `ipc_stt.rs` / `ipc_vad.rs` /
   `ipc_tts.rs`
   - 各リスナー生成にコンテナ名を渡す
6. ドキュメント: `crates/ene-sandbox/src/lib.rs`・`windows.rs`・
   `plans/sandbox-and-downloads.md` の該当箇所を実装後に更新

## 4. 検証方法

### 4.1 クロスコンパイルチェック（Linux でも可能）

flake の dev shell に mingw ツールチェインと rust の
`x86_64-pc-windows-gnu` ターゲットが入っている：

```sh
nix develop --command cargo check -p ene-sandbox --target x86_64-pc-windows-gnu
```

（現状このチェックは通る。実装後も全変更クレートで通すこと。
CI は Linux 専用のため、Windows の実行時検証は実機でしかできない。）

### 4.2 Windows 実機でのスモークテスト

1. ホスト起動 → プラグイン（fs/utility 等）が落ちずに起動し
   `/tool list` に出る
2. プラグインプロセスの整合性レベルが Low であること
   （Process Explorer または PowerShell で確認）
3. ホストサービスパイプ・プラグインパイプの接続が通ること
4. ローカルLLM（llama-server サイドカー）を起動し、ヘルスプローブと
   1ターンの生成が通ること（ループバック免除の確認）
5. プラグインから外部ネットワークへの直接アクセスが拒否されること
6. 設定変更・プラグイン再起動で AppContainer プロファイルが正しく
   再生成されること（プロファイル削除のタイミングに注意）

## 5. 現在のリポジトリ状況（引き継ぎ時点）

- `origin/main` = `488e863b`（未pushだった3コミットは push 済み）
- ブランチ命名: `codex/<topic>`
- 関連PR（すべて open）:
  - #681 コミットメント期限 / #682 レガシーHandshake削除 /
    #683 Assetsサービス削除 / #684 gpu_layers型付き /
    #685 デスクトップ死コード除去 / #686 画面OCR / #687 ヘルス状態一元化 /
    #688 EguiWindowShell / #689 sidecarテンプレート / #690 look_atドキュメント /
    #691 sandboxドキュメント整合 / #692 capabilitiesドキュメント /
    #693 SQLite数学関数（exp）恒久導入
- テスト環境メモ: 素の `cargo test` は `.cargo/config.toml` の
  `LIBSQLITE3_FLAGS` により SQLite 数学関数が有効（#693）。
  ビルドには flake 由来のツール（sccache/clang/mold/cmake/make/pkg-config）と
  `LIBCLANG_PATH` が必要な場合がある。
- 規約: AGENTS.md 参照（Windows はクロスコンパイルのみ・CI は Linux のみ・
  未リリースのため後方互換は不要）
