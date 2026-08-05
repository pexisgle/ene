# メモリ台帳

メモリ台帳は、コンパニオンが覚えているすべてのユーザー向けビューです。
デスクトップの設定 → メモリ台帳ページと、CLI の `/memory`・`/commitments`
コマンドの 2 か所にあります。

## デスクトップの台帳

メモリページには 4 つのタブがあります:

- **ブラウズ** — すべての型付きメモリ。種別/ステータス/スコープで絞り込み、
  インライン編集とピン留めが可能。
- **想起** — クエリに対してハイブリッド検索が返す内容とスコアの内訳
  （なぜこのメモリが関連するか）。
- **保留** — `mind.memory_approval.require_approval` がオンのときに承認待ちの
  候補。メモリになる前に承認・却下・編集できます。
- **約束** — 約束台帳（未完了/完了の約束）。

## CLI

```sh
/memory list
/memory search "キャンプ"
/memory inspect <id>
/memory why <id>               # そのメモリが想起される理由
/memory pin <id>
/memory archive <id>
/memory forget <id>            # ユーザー削除としてマーク
/memory dispute <id>
/memory restore <id>
/memory status
/memory pending
/memory approval list|inspect|approve <id>|reject <id>|edit <id>|history
/commitments list
/commitments done <id>
/affect show
```

## できる操作

| 操作 | 場所 | 効果 |
|---|---|---|
| 保留候補の承認/却下 | デスクトップの保留タブ（ライフサイクルイベントで通知） | 候補がメモリになる、または破棄される |
| メモリの編集 | デスクトップ台帳 / `/memory approval edit <id>` | 内容・種別・重要度・確信度（承認待ち候補） |
| メモリのピン留め | デスクトップ / `/memory pin <id>` | 自然減衰の対象外 |
| メモリの削除 | デスクトップ / `/memory forget <id>` | `user_deleted` としてマーク（監査される）。`/memory restore <id>` で復元 |
| 約束の完了 | `/commitments done <id>` | アクティブプロンプトから外れる |
| 感情のリセット | `/affect reset` | 感情状態をベースラインへ |

## その後のメモリの挙動

- 編集・重要度変更されたメモリは `MemoryLedgerChanged` ライフサイクル
  イベントを発行し、UI が同期します。
- ピン留めメモリは減衰しません。それ以外は減衰します
  （[メモリ → 忘却とライフサイクル](../concepts/memory.md#忘却とライフサイクル)参照）。
- 削除した内容をコンパニオンが後で再抽出すると、新しい候補がパイプラインを
  通って戻ってきます（承認モードをオンにするか、`disputed` の意味論を
  使えます）。

## 承認ワークフロー設定

```json
{
  "mind": {
    "memory_approval": { "require_approval": true }
  }
}
```

承認オンでは、レビューなしに抽出メモリは活性化しません。コンパニオンが
覚える内容を完全に制御したい場合に推奨します。
