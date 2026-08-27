---
name: base-agent-profile-registration
description: Use only when a user explicitly asks to add, list, update, remove, refresh, or discover Codex/Claude account profiles on a Cyborgy worker.
---

# Base Agent Profile Registration

Use this skill only after the user explicitly requests profile management or an authenticated-profile discovery run. Never explore `~/.codex*`, authentication artifacts, or unregistered homes during ordinary startup, worker registration, quota/model refresh, or UI viewing.

## Explicit Registration

- Discover `agent-profile-registry` with `search_apis`; use its `add`, `list`, `update`, and `remove` endpoints through `call_api` with the exact `worker_id`, tool, safe display name, and absolute worker-local home path.
- For explicit Codex discovery, call `discover_and_register_codex_profiles`. The tool invocation itself is the approval for that one bounded run.
- Omit `search_root` to stay under the OS home of the user running that worker. Pass another root only when the user explicitly names and authorizes it.
- Complete the explicit discovery flow through authenticated candidate verification, idempotent registration, and source-refresh request in the same invocation. Report added, existing, skipped, and error counts plus only safe display names and non-secret reasons.

## Safety Boundaries

- Use only the official tool's bounded Codex candidate rules, canonical-path and symlink boundary checks, maximum depth/candidate count, timeouts, and concurrency limits. Do not replace them with unbounded recursive shell searches.
- Authentication must be verified with a short-timeout official CLI status command such as `CODEX_HOME=<path> codex login status`. A matching auth filename is only a candidate hint, never proof of authentication.
- Do not open, read aloud, log, copy, return, or send auth JSON contents, OAuth tokens, API keys, refresh tokens, cookies, or credential values.
- Do not register unauthenticated, ambiguous, unreadable, out-of-boundary, or probe-error candidates. Return only the safe skip reason.
- The canonical identity is `worker_id × tool × profile_id`. Provider email is display metadata and must never merge profiles.
- Never use a profile from another worker or an unregistered path for a Session. Profile, model catalog, resumable conversations, and quota remain scoped to the same registered profile; never add quotas across profiles.
- This skill does not override worker runtime safety, user authorization, filesystem permissions, or the actual authority exposed by the available tools.
