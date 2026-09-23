# Stage 3: 経験の要約と記憶機能

[実装ガイド](../README.md) / [進捗](../PROGRESS.md)

この文書は Stage 3 の**実装順と完了条件**だけを扱います。製品要件は `docs/requirements/`、内部設計は `docs/design/` を優先します。

## 目的

会話から Experience / Memory を形成し、根拠と revision を保ったまま後続会話へ検索・注入できる縦断スライスを作ります。

## 実装順

1. 会話から Experience candidate を抽出する。
2. Experience Summary を生成し、根拠となる source との対応を保持する。
3. Companion scope の Memory を形成・更新する。
4. 「当初の認識が誤っていた訂正」と「状況が後から変化した更新」を区別し、過去 revision を保持する。
5. 過去の Experience / Memory を検索し、context assembly を経て次の会話へ組み込む。
6. 管理経路から Memory とその根拠を読み出せるようにする。
7. formation → correction/update → recall → restart を縦断テストで固定する。

## 完了条件

- 会話から新しい Memory が形成される。
- 訂正・状況変化・revision の履歴が区別される。
- 再起動後も検索結果を使って自然な応答を生成できる。

## フォローアップ

学習用モデルの設定、形成方式、設計上の correction interface と現在の formation 内包経路の型分離は [Learning の後続計画](../follow-ups/learning.md) で扱います。Stage 3 の完了履歴と、未解決の後続作業を混同しません。
