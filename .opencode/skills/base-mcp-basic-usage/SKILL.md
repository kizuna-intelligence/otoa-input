---
name: base-mcp-basic-usage
description: "Official Cyborgy base skill describing basic MCP usage for Role and Skill Set settings, API scope, worker coordination, and plan, progress, and completion reporting."
---

# Base MCP Usage

Use Cyborgy MCP as the first-class interface for Cyborgy-managed skills, Agent Settings, APIs, worker coordination, and plan, progress, and completion reporting.

## Agent Settings And Workspace State

- Discover `agent-settings`, then use
  `call_api(api_id="agent-settings", endpoint_id="get")` to inspect the single
  Base Skill Set, the caller's Organization Skill Set, and Repository Skill
  Sets.
- Replacement writes require read-then-write. First call `get` and read the
  current direct Skill IDs, then add/remove the requested IDs locally, then
  send the complete list with `expected_organization_skill_ids` copied from the
  list returned by `get` in
  `call_api(api_id="agent-settings", endpoint_id="set_organization",
  inputs={...})`. The expected list is checked in order. If it changed, the
  write returns `409 Conflict` with the current list; re-read it and retry.
  Never send an organization replacement without reading first: because this
  API replaces the whole list, omitted existing Skills are all deleted.
  `organization_skill_references` uses the same expected-ID protection. There
  is no purpose-specific Worker Manager, Developer, Task Reviewer, or Code
  Reviewer Skill Set. A successful write that removes Skills reports their
  IDs in `removed_organization_skill_ids`.
- Use the same mandatory read-then-write sequence for
  `call_api(api_id="agent-settings", endpoint_id="set_repository",
  inputs={...})`: read the repository's current `skill_ids`, make the change,
  and send the complete list with `expected_skill_ids`. Repository settings
  are selected automatically from the current folder's Git remote; do not
  create folder assignments or an explicit Git-to-Skill-Set link. Omitting
  existing IDs from a replacement deletes them; successful deletions are
  reported in `removed_skill_ids`.
- Use `get_workspace` to inspect the detected GitHub repository and the Agent Settings resolved for the current folder.
- Use `sync_workspace` when Agent Settings may have changed and the current
  folder should materialize the latest resolved Skills.
- Use `search_apis`/`call_api(api_id="online-workers", inputs={"online_only": "true"})` before any operation that requires a Cyborgy `worker_id`. Pass the exact returned worker `id`; do not substitute an agent ID, session ID, machine ID, hostname, or process ID.

## Skills

- Discover the `skill-manager` API with `search_apis` and use its `search` and
  `owned` endpoints to discover available or caller-owned skills and resolved
  Skill Sets.
- Search before creating a new skill to avoid duplicates.
- Use the `skill-manager` `files` endpoint to read a skill's actual `SKILL.md`
  and resources before referencing or modifying it.
- For a new private skill, call `create` for its metadata, then immediately call
  `put_files` with the complete file list including a valid `SKILL.md`. An
  omitted or empty `expected_head_sha` is valid only for that brand-new skill
  before its first file version exists.
- Use its `update`, `restore`, and `put_files` endpoints for edits. Read the
  current file set and pass its exact non-empty `expected_head_sha` for
  optimistic concurrency;
  do not silently replace files omitted from an intentional full-tree update.

## APIs

- Use `search_apis` to find callable deployed APIs.
- Inspect an API's schema before calling it.
- Session user rules remain trusted runtime operations exposed as first-class
  MCP tools. Agent quota and Session compact display are Catalog APIs: discover
  `tool-quotas` and `session-compact-mode` with `search_apis`, inspect their
  endpoint schemas, and invoke them through `call_api`.
- Worker availability is a Catalog API. Find and call it with
  `search_apis`/`call_api(api_id="online-workers", inputs={"online_only": "true"})` instead of a direct MCP tool.
- Do not manually widen `.cyborgy/settings.json` unless the user explicitly asks.

## Coordination, Progress And Completion

- Before push, deploy, Terraform, GPU work, or other conflict-prone shared-resource use, check and post to the AI work-report / conflict-prevention channel when available.
- At the beginning of every user-requested work turn, call `display_work_plan_to_user` before substantive work with a concise one-line task summary for the session title and the current plan. Call it again whenever the plan changes so the newest plan is durably recorded and displayed; the Workers grid shows whichever is newer—the latest plan or progress report.
- Treat intermediate progress reporting as required for work with more than one substantive phase, not as an optional courtesy. Call `display_work_progress_to_user` after each meaningful phase boundary (for example, after investigation or a decision, and before verification when those phases occur), and before a long wait. A one-step request that completes in one action may use the plan and completion reports only. Do not wait for `notify_work_complete` to report an intermediate result.
- `display_work_progress_to_user` records a progress checkpoint for the current Session and updates its user-visible Workers grid overlay. It does not send agent/session input, create a Task or session, change execution status, or update a planning Task.
- Write the plan, progress, and completion text in the user's language. Lead
  with the user-visible outcome, remaining work, and next action; do not use
  Task IDs, Session IDs, API IDs, revision names, or implementation terms as
  the explanation.
- At the end of every Cyborgy worker turn, call `notify_work_complete` with a concise one-line summary, detailed summary, and `complete` or `user_waiting`. Follow `base-work-completion` for the wording and state decision: waiting is only for unfinished work that cannot continue without an immediate user answer or action.
