# プラグイン合成(時空間)

> 実現する要件: **P-1010**(時空間合成)、P-1002 の実行時面、P-1005 の
> ライフサイクル面。参照: D-32。
> 形式化の出典: Shi, Zhang, Cui, *A Programming Paradigm for Spatiotemporal
> Composability*(Cordis)。採用するのは2次元の抽象だけで、
> TypeScript の in-process ローダそのものは採用しない
> ([../product/vision.md](../product/vision.md) §4)。

アウトプロセスのプラグインは子プロセスとして隔離する(D-10)。
それだけではホスト文脈(ツール登録・seam 束縛・Broker 委譲)は回収されない。
この文書は、ホスト側の合成を**時間**(巻き戻し)と**空間**(依存の再評価)の
2次元で閉じる。輸送は [ipc.md](ipc.md)、宣言は
[manifest-and-profile.md](manifest-and-profile.md)、資源の仲介は
[broker.md](broker.md)。

## 0. プロセス kill では足りない理由

OS のプロセス境界は、プラグイン側のメモリを捨てる。ホストに残るものは捨てない。

| 残るもの | kill したあと |
|---|---|
| 統一レジストリのツール行 | 死んだ供給元のスキーマが表層/裏層に残り続ける |
| provider seam の束縛 | `ai.tasks.*` が幽霊プロバイダを指す |
| MCP 由来の Context Source | 組立が Unavailable でも行が残る |
| Broker の grant / サイドカー | FD と子エンジンが回収されない |

これを避けるためにコアデーモンごと再起動すると、会話・音声パイプライン・
進行中 job のプロセス内状態まで捨てる。論文 §1.2.3 が斥ける
coarse-grained workaround であり、ハーネスが実行中に行を足し引きする
(プロファイル切替、プロバイダ死亡、コミュニティプラグイン導入)用途と衝突する。

対象は**アウトプロセスのツールとプロバイダ**。ハーネス機能ツールは
ホスト内で、カーネルの寿命に紐づくのでこの合成の対象外
([../tools/registry.md §0](../tools/registry.md#0-2軸の分類d-10))。

## 1. システム境界

巻き戻しの範囲は境界で決まる。境界の内側だけを追跡する。

| 側 | 例 | ファイバー停止時 |
|---|---|---|
| **内側**(追跡する) | ツール/seam/Source の登録、Broker の grant(取得)、サイドカー handle、プラグインプロセスそのもの、子ファイバー | 逆を適用して回収する |
| **外側**(追跡しない) | ユーザー可視ファイルへの書き込み、既に送った HTTP、既に消費した LLM 呼び出し、セッションログの追記 | 回収しない。ログの追記を戻すと I-2 に反する |

外側への放出をどうしても戻す必要があるときは補償であり、合成の逆ではない。
`fs.undo` は当該 job が書いたものだけを戻す補償(D-30)で、プラグイン停止の
自動効果にはしない。停止したからといってユーザーの成果物を消さない。

Broker の2段(論文 §6.1 の acquisition / emission)に対応させると、
`fs.open_write` で渡した FD の close は内側、その FD へ書いたバイトは外側。

## 2. ファイバー

プロファイルの**行**が実行実体になる。パッケージではない。
同じプラグインを設定違いで2行置ける。

| 欄 | 内容 |
|---|---|
| `uid` | 起動ごとに払い、再利用しない。差し替え前後を値の一致で取り違えない |
| `plugin` / `version` / `config` | 行の識別。config の要約は reconcile の比較に使う |
| `requires` | ACTIVE になるために必要な鍵(§4) |
| `provides` | このファイバーが入れる鍵。ツール名は `spec` 到着後に確定してよい |
| `state` | `inactive` / `loading` / `active` / `unloading` / `failed` |
| `target` | いま解決している requires の提供者 `uid` の組。変化が再評価の合図 |
| `dispose` | 追跡した逆の蓄積。適用は LIFO |
| `process` | 子プロセス。無い(`inactive`)こともある |
| `inertia` | 進行中の遷移。入ったら完了まで次の遷移を始めない |

行の挿入自体が親(プロファイル/グループ)の effect である。親を卸すと子へ連鎖する。

状態の読み:

| 状態 | 意味 |
|---|---|
| `inactive` | プロセスなし。ホスト文脈にこの行の登録なし。requires 待ちを含む |
| `loading` | spawn〜登録の途中。部分失敗は蓄積した逆で回収して `failed` または `inactive` |
| `active` | `provides` が他ファイバーから見える。requires の解決を `target` に固定 |
| `unloading` | 提供を**直ちに**止める。逆の適用は、依存側が `inactive` になってから |
| `failed` | サーキットブレーカーが開いた行。手動再起動まで `inactive` 相当 |

`unloading` に入った瞬間に提供を止めるのは、依存側が「まだ使える」と
読み続けないため。逆の適用を先にすると、依存側の teardown が欠落した鍵を読む。

## 3. 巻き戻し可能な effect(時間)

ホスト文脈を変える操作は、必ず逆と対にする。SDK が対を組み立て、
作者は適用側だけを書く。VSCode 型の `deactivate` は置かない。

| 適用 | 逆 |
|---|---|
| ツールをレジストリへ載せる | 同じ名前を降ろす |
| provider seam に束縛する | 束縛を外す |
| Context Source を載せる | Source を外す(`render_removal` が描画する) |
| Broker grant を渡す | FD/handle を回収する |
| サイドカーを spawn する | kill する |
| プラグインプロセスを spawn する | drain → 猶予 → kill |
| 子ファイバーを挿入する | 子を卸す |

合成の規則:

- 逆の蓄積は LIFO。1ファイバー内の依存順(先に登録したツールを後から使う)を
  作者が意識しなくてよい。
- 異なるファイバーの登録は、鍵が衝突しなければ交換可能。衝突した
  後着は `failed`(先着が鍵を保持)。名前空間
  (`<namespace>.<action>` / `<kind>.<name>`)が衝突面を狭める。
- `loading` の途中で失敗したら、そこまで追跡した逆だけを適用する。
  半登録を残さない。
- プロセスが突然死んでも、ホスト側の逆は走らせる。OS がプロセスを
  回収済みなら spawn の逆は no-op で、登録と grant の逆は通常どおり。

ハーネス機能ツールと waterfall リスナーはカーネルが所有し、
サードパーティファイバーからは登録できない。ループの拡張点を
アウトプロセスに開けない(承認・quiet hours の横取りを防ぐ)。

## 4. 反応的な依存(空間)

ホスト文脈は鍵→値の表である。ファイバーは `provides` で入れ、
`requires` がすべて揃ったときだけ ACTIVE になる。
変化は毎回、各ファイバーの `requires` に対して
activating / deactivating / neutral に分類する。

### 4.1 鍵の家族

| 家族 | 入れる主体 | 読む主体 | 衝突 |
|---|---|---|---|
| `tool.<namespace>.<action>` | ツールプラグイン(`spec`) | レジストリ、層ごとのスキーマ面 | 後着は `failed` |
| `seam.<name>` | **ホスト**(ブローカー常駐) | ホスト、およびその seam を `requires` するファイバー | ブローカーは常駐。backing は複数可 |
| `broker.<op>` | ホストの Broker(常駐) | `[capabilities]` を宣言したファイバー | 常駐 |
| `mcp.<server>` | MCP ブリッジ行 | レジストリ、`mcp.resources` Source | サーバー名で後着 `failed` |

`[capabilities]` は `broker.*` に対する `requires` である
([manifest-and-profile.md](manifest-and-profile.md))。
宣言していない Broker 操作は `denied`(P-902)。

`[provides].seams` は `seam.*` を独占する宣言ではない。
そのブローカーへ **backing として登録する**宣言である。
登録はファイバーの effect で、unload すると経路集合から外れる。

### 4.2 プラグイン同士は話さない

空間合成の配送はホストの seam が行う。プラグインプロセスから
別プラグインのソケットは見えない(サンドボックス、
[../security/sandbox.md](../security/sandbox.md))。

論文の service broker(複数 provider が共存し、入口だけが安定する形)は、
既存の capability seam 3役割そのもの
([../tools/capabilities.md](../tools/capabilities.md))。

- 消費者は `seam.llm` を `requires` する。個別のプロバイダ行を
  `requires` しない。
- `seam.llm` の鍵はホストが持つ。`provider.openai` も
  `provider.local-llm` も backing として共存できる。
  `ai.tasks.<task>` が選ぶ。
- `requires.seams = ["llm"]` の充足は「ブローカーに ACTIVE な backing が
  1つ以上ある」。最後の backing が落ちると deactivating。
  backing の差し替え(1つ以上残る)は neutral で、消費者ファイバーは
  reload しない。
- `fs` は `seam.llm` を require しないので、LLM プロバイダの死で動かない。
- 排他束縛(鍵を1 provider だけが `set` する形)はツール名と MCP サーバー名に
  限る。プロバイダ seam では使わない。

### 4.3 活性化と撤退の順

1. 提供者のファイバーが `unloading` に入る(提供停止。逆はまだ走らない)。
2. その鍵を `requires` するファイバーが deactivating と分類され、
   自分の `unloading` に入る。
3. 依存側がすべて `inactive` になってから、提供者の `dispose` を適用する。
4. 循環(`A` が `B` を require、`B` が `A` を require)は宣言から検知し、
   関係する行をすべて `inactive` のまま報告する。実行時デッドロックにはしない。

撤退待ちには上限を置く(数値は設定キー、D-29)。上限を超えた依存側は
強制 `inactive` にし、提供者の回収を止めない。コアデーモンを
依存側の暴走で止めないため。

`requires` が満たされない行はエラーにせず `inactive` で待つ。
プロバイダが後から来れば activating で `loading` に入る。

## 5. 隔離と interception

論文の isolation / interception は、Ene では新しい IPC 面ではなく
既存の Broker・サンドボックスに対応する。

| 抽象 | Ene での実現 |
|---|---|
| 隔離レルム | ファイバーごとに Broker の grant 表が独立。A の FD を B は見ない |
| interception メタデータ | `[capabilities]` の `scope` と承認ポリシー。ホストが呼び出しごとに照合 |
| 未宣言アクセスの拒否 | 宣言外の Broker 操作は即 `denied`。hello で付与した `capability_ref` 以外は使えない |

interception の変更(ポリシーの scope 縮小など)は満足条件を変えないので、
ファイバーの reload を起こさない。隔離レルムの付け替えは対象鍵の
提供者判定が要るので、その行を rebuild する
([manifest-and-profile.md §2](manifest-and-profile.md))。

サンドボックスは言語外の実行境界である(論文 §6.3)。
`requires` の静的審査は「何を要求するか」を起動前に見せるだけで、
悪意あるネイティブコードの代わりにはならない。両方要る。

## 6. ライフサイクル(慣性)

遷移に入ったら、完了するまで次の遷移を始めない。完了時に `target` が
変わっていれば、反対側へ連鎖する。

```text
inactive ──(requires 充足)──► loading ──(apply 成功)──► active
                ▲                 │                        │
                │            (失敗/target 変化)             │
                │                 ▼                        │
                └──────── unloading ◄──(requires 喪失 / disable / 死亡 / rebuild)
```

- **reload**: spawn → ハンドシェイク → apply(登録)。途中で `target` が
  崩れたら成功扱いにせず `unloading` へ連鎖。
- **unload**: 提供停止 → 依存側の `inactive` 待ち → `dispose`(LIFO) →
  `inactive`。`target` がまた充足していれば `loading` へ連鎖。
- **死亡**: ping 不達/EOF。spawn の逆は no-op、その他の逆は通常の unload。
  監督が指数バックオフで reload を試み、規定回数で `failed`
  ([../platform/process-model.md §3](../platform/process-model.md#3-プラグイン監督p-1005))。

`drain` はプロセスの逆の前段([ipc.md §4.1](ipc.md#41-core))。
ホスト文脈の逆(登録を降ろす)は `drain` と独立に、必ず走らせる。

## 7. 障害モード

| 障害 | 挙動 |
|---|---|
| `requires` 未充足 | `inactive` で待つ。ライフサイクルイベントで不足鍵を報告。ツリー全体は継続 |
| `requires` の循環 | 関係行をすべて `inactive`。起動時に報告 |
| `provides` の鍵衝突 | 後着を `failed`。先着が保持 |
| `loading` 途中の失敗 | 追跡済みの逆を適用。半登録を残さない |
| プロセス突然死 | ホスト側 unload。監督が再起動または `failed` |
| 依存側が unload 待ちで停滞 | 上限後に強制 `inactive`。提供者の回収を優先 |
| 逆の適用失敗 | 残りの逆は続行。監査ログに「回収失敗」。連続失敗は `failed` |
| 外側への放出(書き込み済みファイル) | 回収しない(§1)。`fs.undo` は job の補償 |

## 8. 設定キー

数値は実装しながら決める(D-29)。

| キー | 説明 |
|---|---|
| `plugins.fiber.unload_wait` | 依存側 `inactive` 待ちの上限 |
| `plugins.fiber.apply_timeout` | `loading` の apply 期限(ハンドシェイク後の登録完了) |

監督のヘルス・ブレーカーは
[../platform/process-model.md](../platform/process-model.md) のキー。

## 9. 後継に残すもの

- **ネイティブバイナリの HMR**。モジュールキャッシュの無効化は
  管理ランタイム向け。v1.0 はファイバー rebuild(そのプロセスだけ建て直し)で足りる。
  形式が支えるのは「unload がホスト文脈を空に戻す」こと。
- **プラグインが別プラグインの提供する独自鍵を直接 require する開いた生態系**。
  v1.0 の `requires` はホスト seam と Broker に限る。独自鍵を開けると
  版と衝突の問題(論文 §6.6)がコミュニティ前提(D-26)より先に来る。
  鍵の名前空間(`<kind>.<name>`)と副プロトコル版だけ先に固定する。

---

- 前: [ipc.md](ipc.md) / 次: [manifest-and-profile.md](manifest-and-profile.md)
