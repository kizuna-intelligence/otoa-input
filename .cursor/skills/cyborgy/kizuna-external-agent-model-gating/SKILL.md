---
name: kizuna-external-agent-model-gating
description: Kizuna Intelligence Developer ロール専用。実装変更を機能スコープで分類し、単一機能はDevinまたはGPT-5.3 Codex Spark、複数機能はLuna mediumまたはClaude Sonnetへ割り当てる。cursor-agentで使ってよいモデル、quota不明時の起動fallback、Grokの利用条件も定める。
---

# Kizuna External Agent Model Gating

Kizuna Intelligence の開発者ロールで、外部の自律コーディングエージェントを
サブエージェントとして呼び出す際の選定条件。

## 実装前提

- 要件、設計、受け入れ条件が未確定なら、実装エージェントへ直接渡さず、先に設計を確定する。
- 実装を依頼するときは、ファイル数だけでなく、変更がまたがる機能境界を確認する。
- Worker、tool、正確なmodel ID、profile、必要なcapabilityは起動直前の実データで確認する。model IDを推測しない。

## 単一機能の変更

次を満たす変更は、単一機能の変更として扱う。

- 一つのプロダクト機能、一つのvertical feature、または一つの明確な責務の範囲に収まる。
- 要件と実装方法がほぼ確定しており、他の独立した機能や公開contractとの調整が不要である。
- おおむね5ファイル程度までのコンパクトな影響範囲は判断材料になるが、ファイル数だけでは分類しない。

候補は次の順に選ぶ。

1. Devin（`swe-1-7`）
2. Codex GPT-5.3 Codex Spark（Workerがadvertiseする正確なID。通常は `gpt-5.3-codex-spark`）

Devinは通常の実装候補として利用してよい。以前の隔離起動検証や、同一Session・一件だけに限定する条件は適用しない。

## 複数機能の変更

次のいずれかに該当する変更は、複数機能の変更として扱う。

- 二つ以上のプロダクト機能またはfeature boundaryを協調して変更する。
- Backend、Web、CLI、Worker protocolなど、独立したcomponentや公開contractをまたぐ調整が必要である。
- 一つの変更でも、複数の責務や利用者フローに別々の判断が必要である。

候補は次の順に選ぶ。

1. Codex Luna（`gpt-5.6-luna`、`thinking_depth=medium`）
2. Claude Sonnet（Role設定では `claude-sonnet-5`。起動時はWorkerがadvertiseする正確なID。通常は `sonnet`）

## cursor-agentで使ってよいモデル

`cursor-agent` を使うときは、次のいずれかだけを指定する。

- **Cursor Grok 4.5** — `cursor-grok-4.5-high`（または `cursor-grok-4.5-high-fast`）
- **Composer** — `composer-2.5`

**これ以外のモデルを勝手に使わない。**

`cursor-agent --list-models` は Codex 5.3 系、GPT-5.2、Claude Opus 5 など多数を
advertiseするが、それらは選択肢ではない。`auto` も使わない。既定のまま起動すると
`auto` になるため、**モデルを必ず明示する**。

上記2つで要求を満たせないと判断した場合は、勝手に別モデルへ切り替えず、
理由を示してユーザーの判断を仰ぐ。

## Quota不明時のfallback

- 起動直前にquotaを確認し、明示的な exhausted、unavailable、error は候補から除外する。
- quotaがunknown、stale、未取得、または表示されない場合、それだけを理由に候補から除外しない。Workerと正確なtool/modelがadvertiseされていれば、不確実性と次候補を記録して一度通常起動する。
- `wait_agent_session_start` で確認し、`native_started` より前にerror、timeout、stop、または `agent_startup_stalled` になった場合は、実エラーを記録して同じ分類の次候補へ切り替える。
- 証拠が変わらないまま同じ候補を繰り返し起動しない。
- `native_started` 後、または実装開始後は、重複実装を避けるためSessionとcheckoutの状態を確認してから再割り当てする。

## Grokの許可条件

対象は `grok`（モデル `grok-4.5`）。

以下のどちらかを実行モデルとして使っている場合のみ、Grokを利用してよい。

- `claude` / `fable` (Claude Fable)
- `codex` / `gpt-5.6-sol` (Codex Sol)

それ以外の実行モデルのセッションではGrokをサブエージェントとして起動しない。
呼び出しが必要に見える場合は、まずこの条件を満たすモデルに切り替えられるか
確認し、無理なら通常の手持ちツールで代替する。

これは `grok` ツールの条件であり、`cursor-agent` の Cursor Grok 4.5 とは別である。

## スコープ

この条件は Kizuna Intelligence Developer ロール（および配下の製品別
Developer ロール）にのみ適用する。レビュー・監督・ワーカーマネージャー系の
ロールにはGrokの許可を広げない。実装エージェントの選定では、上記の機能スコープ、
起動時の実データ、およびquota fallbackを適用する。
