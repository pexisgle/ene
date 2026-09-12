# 参考資料

ステータス: **参考資料（仕様を決定するものではありません）**

このドキュメントでは、ene の要件や設計を検討するにあたって参考にした既存の製品、オープンソースプロジェクト、標準規格と、その採用理由をまとめています。

外部サービスの仕様変更や本ドキュメントの記述が [製品要件 (requirements.md)](requirements.md) と矛盾する場合は、常に製品要件の記載を優先します。

> [!NOTE]
> ここで「採用」としているのは、設計思想やユーザー体験を参考にしたという意味であり、既存ツールのコードをそのままコピーしたり、全く同じ画面構成にしたりすることを意味するものではありません。（確認日: 2026-09-05）

## コンセプトの主な参考元

ene は何か単一の製品を真似たものではなく、既存の優れた複数の製品から異なる強みを組み合わせています。
大きく分けると、以下の3つの潮流を融合した点に ene の独自性があります：

1. **自律作業 AI**: Grok Bot / OpenClaw / Claude Cowork / ChatGPT Work
2. **エージェント実行基盤 (Agent Harness)**: OpenCode / Hermes Agent / nanobot
3. **パートナー・キャラクター体験**: AIRI / Desktop Mate / Nomi / Kindroid

| 参考プロジェクト | 参考にした中心的な考え方 | ene における違い・特徴 |
|---|---|---|
| [Grok Bot](https://x.ai/bot) | 専用PCを持つAIチームメイトに実作業を任せ、ブラウザ操作やルーチンワークを並列実行できる体験。ene のエージェント体験に最も近い参考元。 | クラウド上の専用PCや複数の人格を前提とせず、ユーザーの手元PC上でパートナー（キャラクター・記憶・感情・3Dアバター）と実作業能力を一体化。 |
| [OpenClaw](https://github.com/openclaw/openclaw) | ユーザー自身のPCでエージェントを常駐させ、ツールやスキルを使って自律的に作業させるセルフホスト（自己管理）モデル。 | PC操作、タスク、ワークスペース、認証情報、権限境界を厳密に整理し、それらの作業能力を長期的なパートナーから利用できるように統合。 |
| [Claude Cowork](https://support.claude.com/en/articles/13345190-get-started-with-claude-cowork) | AI にPC上のフォルダやファイルを渡して、作業の自動化を任せるワークスペースモデル。 | 一時的な作業エージェントを独立した人格にはせず、パートナーから呼び出される「作業の手足」として位置づけ。 |
| [ChatGPT Work](https://openai.com/ja-JP/chatgpt-work/) | 会話を通じて複数ステップの作業や成果物作成をAIに任せる体験。 | クラウド依存ではなく、ユーザーの手元PC上で安全な権限管理のもと完結させるローカルファーストな基盤へ統合。 |
| [OpenCode](https://dev.opencode.ai/docs/agents/) | モデル、ツール、エージェント、権限などをきれいに分離した設計。 | コーディング専用のエージェントに閉じず、パートナーとの日常会話やPC操作、スケジュール実行など幅広く使える共通基盤へ汎用化。 |
| [Hermes Agent](https://github.com/NousResearch/hermes-agent) | エージェント実行ループ、メモリ、スキル、サブエージェントなどの総合設計と、AI自身が知識や手順を学習・改善していく考え方。 | 各モジュールを疎結合に保ちつつ、AIの自己改善についてもユーザーの承認や安全管理の枠組み内に収める。 |
| [nanobot](https://github.com/HKUDS/nanobot) | 最小限のシンプルな部品で軽量にエージェント基盤を構成するアプローチ。 | 不要な固定レイヤーや独自プロトコルを増やさず、シンプルで無駄のない構成を保つための比較対象として参考。 |
| [AIRI](https://github.com/moeru-ai/airi) | AIを所有可能なキャラクターとして提示し、音声、アバター、画面認識、PC操作を1つの存在にまとめる方向性。 | キャラクター体験だけでなく、Grok Bot/OpenClaw系のような自律作業能力や本格的な長期記憶を同じ個体に統合し、ローカルPCをマスターとする。 |
| [Desktop Mate](https://store.steampowered.com/app/3301060/Desktop_Mate/) | デスクトップ上にキャラクターが常駐し、普段のPC作業と自然に共存するUI/UX。 | 単なる観賞用のマスコットにとどまらず、実際の会話、記憶、ファイル操作タスクまでを同じパートナーに統合。 |

## キャラクター・アバター・会話体験

| 参考プロジェクト | 参考にした考え方 | ene における違い・特徴 |
|---|---|---|
| [VRM 1.0](https://vrm.dev/en/vrm1/) | 人型3Dアバターの標準形式。 | ene 独自のモデル形式は作らず、公式アバター形式として VRM 1.0 をそのまま採用。 |
| [Nomi: Getting started](https://nomi.ai/nomi-knowledge/nomi-101-a-beginners-guide-to-getting-started-with-your-ai-companion/) | キャラクターを出発点として、継続的な対話を通じて関係性を築く体験。 | クラウドサービス上のアカウントではなく、手元のPCを大元とし、PC上の実作業能力も統合。 |
| [Kindroid: Customizing personality](https://kindroid.ai/v2/docs/customizing-personality/) | キャラクターの初期設定を出発点として編集可能にする仕組み。 | 日々の経験から学んだ記憶と、初期の静的な設定を明確に分離。キャラクターの更新で学習した記憶を上書きしない。 |

## 会話・記憶 (Memory)・関係性 (Relationship)

| 参考プロジェクト | 参考にした考え方 | ene における違い・特徴 |
|---|---|---|
| [Nomi: Long-term memory in group chat](https://wiki.nomi.ai/Long_term_memory_in_group_chat) | 一対一の対話とグループ会話で、パートナーごとの記憶の境界を保つ。 | 特定パートナーとの思い出はそのパートナー専用の記憶とし、全体で共有すべきものだけをグローバル記憶とする。タスク固有の情報は作業領域に分離。 |
| [Hermes Agent: Persistent Memory](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/features/memory.md) | 長期記憶を生ログとは分け、後で役立つ要約情報に絞り込む。 | 会話の要約（Experience Summary）を根拠として、AIが自然に長期記憶を形成・更新する。 |
| [nanobot: AI Agent Memory](https://github.com/HKUDS/nanobot/blob/main/docs/guides/ai-agent-memory.md) | セッション履歴と厳選された長期記憶を分ける。 | 会話履歴、経験の要約、学習した記憶をそれぞれの役割ごとに明確に分離。 |
| [Mem0: Add / Update Memory](https://docs.mem0.ai/) | 会話から有用な情報を抽出し、重複や矛盾を考慮して既存記憶を更新する。 | 単純な上書きだけでなく、「誤りの訂正」と「以前は正しかったが状況が変わったこと」を区別し、過去の変更履歴も保持する。 |
| [Kindroid: Learned Context](https://kindroid.ai/v2/docs/chat-features-and-tools/) | 関係性や重要事実を、会話の進展に合わせて更新されるメモとして保持。 | 事実の記憶（Memory）と、相手に対する印象・関係性（Relationship）を役割分担し、情報の二重管理を防ぐ。 |
| [Replika: Deletion & Chat history](https://help.replika.com/) | チャット履歴の削除と、学習済みメモリの管理を分けて扱う。 | 日常の自然な忘却（思い出しにくくなること）と、セキュリティ・プライバシー目的の完全削除（targeted deletion）を明確に区別。完全削除時は関連する要約や派生データまで徹底的に消去する。 |

## タスク管理・作業委任・スキル

| 参考プロジェクト | 参考にした考え方 | ene における違い・特徴 |
|---|---|---|
| [Model Context Protocol (MCP)](https://modelcontextprotocol.io/) | 外部ツールやリソースをプロバイダー非依存で接続する標準仕様。 | MCP を採用しつつ、API キーなどの認証情報がプロンプトやモデルに漏洩しない安全境界を厳格に保持。 |
| [Agent Skills specification](https://agentskills.io/) | 人間が読める手順や知識を再利用可能なスキルとして配布する標準。 | 独自形式に閉じず Agent Skills との相互運用性を重視。スキルを経験から学習・改善する仕組みも提供。 |
| [AGENTS.md](https://agents.md/) | 作業フォルダ内のテキストファイルでエージェントへの指示を共有する標準的な慣習。 | フォルダごとの指示書をそのまま自然に読み込み、タスク実行に活用。 |

## セキュリティ・権限・ローカルファースト

| 参考プロジェクト | 参考にした考え方 | ene における違い・特徴 |
|---|---|---|
| [OWASP: Prompt Injection 対策](https://genai.owasp.org/llmrisk/llm01-prompt-injection/) | 外部データに含まれる悪意ある指示と、ユーザー本人の指示を明確に分ける。 | プロンプト側の防御だけに頼らず、システム的な権限チェックとユーザーへの確認を併用して安全を確保。 |
| [Android ストレージ分離モデル](https://developer.android.com/training/data-storage) | アプリ内部データと、ユーザーが自由に扱う通常ファイルのライフサイクルを分ける。 | ene の内部データ（記憶や設定）と、作業対象となるユーザーのファイルフォルダ（Workspace）を明確に分離。 |
| [Tailscale](https://tailscale.com/) | ユーザー自身のVPNを通じて端末間を安全に接続する。 | 特定のVPNサービスに依存せず、独自のクラウド中継サーバーも置かないプライベートな接続モデルを採用。 |

