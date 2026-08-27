---
name: base-agent-cleanup
description: Use in Worker Manager sessions when reducing the worker grid by inspecting current session facts and deleting exact user-directed targets with an inspect/compare-delete integrity fence.
---

# Base Agent Cleanup

Use this skill when a Worker Manager reconciles worker sessions from current facts, bounded canonical logs, and explicit user intent.

## Inspect and Compare-Delete

- Discover `session-cleanup` with `search_apis`, then call its `inspect` endpoint for the specified worker. It returns one stable page without completion filtering, including `id`, `worker_id`, `pinned`, `status`, `kind`, `completion_status`, Task reference/PR state, `latest_seq`, and a bounded latest canonical log. Continue passing the opaque `next_cursor` as `cursor` while `has_more` is true; do not treat the page size as an exhaustive result.
- Read current facts and latest log evidence, then choose exact target IDs from the user’s intent. The backend returns facts and does not choose, rank, or infer candidates.
- Call the `session-cleanup` Catalog `delete` endpoint with the same `worker_id` and one `{id, expected_latest_seq, expected_pinned: false, expected_status}` entry per exact target, copying `expected_latest_seq` and `expected_status` from the latest inspection. `expected_status` must be the exact inactive state observed for that Session: `idle`, `done`, `deferred`, `stopped`, `error`, or `disconnected`. The Backend rejects active statuses and any target that does not explicitly require `expected_pinned: false`. These expected fields are compare-and-delete integrity/TOCTOU fences, not cleanup policy.
- If an explicitly user-directed target is active, perform only the separately authorized lifecycle transition, then re-inspect it and construct a new delete target from the resulting inactive facts. Deletion intent alone is not permission to stop active work.
- Use `dry_run=true` only when an exact target/sequence preview is useful. It returns auditable target, deleted, skipped, and error results and does not issue confirmation tokens. The destructive call uses the unchanged exact target set and current sequences.
- Treat a stale sequence, foreign worker, missing target, malformed ID, duplicate target, or persistence failure as an auditable skip/error and investigate or re-inspect as appropriate. Do not replace this contract with a generic session-delete call.

Report inspected, deleted, skipped, and error counts after each cleanup sweep, including the exact target IDs and sequence evidence used.
