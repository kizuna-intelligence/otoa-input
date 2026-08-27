---
name: base-task-defaults
description: Use whenever creating, moving, updating, attaching files to, or reconciling a planning and record Task and its project, hierarchy, planning dependencies, pull requests, comments, or events.
---

# Task planning and records

Ordinary Task creation requires an explicit existing `project_id`. Never
create, select, or use a Default project as a fallback. An omitted parent on
create means the root of the explicitly selected project; parent changes,
cross-project moves, and deletes must use exact Task IDs and preserve the
operation-correlated event/Task Tree history of their atomic side effects.

Treat Tasks as project-scoped planning and record resources. Preserve the
selected project, parent-child display hierarchy, planning dependencies,
files, pull requests, comments, and append-only events.

Tasks do not own Session assignments, Work Graphs, scheduling, execution
assignment, claims, retries, or executor selection. A planning dependency
records a real prerequisite but never grants authority or starts, blocks,
routes, retries, reviews, or completes execution.

## Tool selection

Resolve a Task or Storage route in this order:

1. An explicit instruction in the current user request.
2. The latest relevant Task or parent-Task comment that names a tool.
3. The standard Catalog API exposed by the current runtime.

For the standard Task route, discover `task-manager` with `search_apis`, inspect
the selected action schema, and invoke it through `call_api`. For Task file
bytes, discover `task-files` and invoke its upload/download endpoint through
`call_api`; the generic runner preserves the file-safety contract. Do not
assume the former individual Task MCP wrappers are present.

Use comments such as `task-tool: <tool>` or `storage-tool: <tool>` when a Task
needs a non-standard tool. Use the selected tool's current schema. Do not
hard-code an API ID, provider route, or credential. If a selected custom tool
is unavailable, report that exact blocker rather than silently choosing
another tool.

## Standard Task procedure

1. List the visible Task projects and resolve the intended project by exact ID.
2. Create the Task with that project ID and the requested parent, summary,
   planning dependencies, priority, and due date.
3. Read the created Task and verify its project, parent, and requested fields.

## Standard Storage procedure

1. For a Task attachment, discover `task-files`, then upload the local file
   through `call_api(api_id="task-files", endpoint_id="upload", inputs={...})`
   using the exact Task ID.
2. List that Task's files and verify the returned file ID is attached.
3. For standalone Storage, discover `storage`, inspect the selected endpoint's
   schema, invoke it through `call_api`, and verify it with a read or list.
4. Keep Session delivery, standalone Storage, and Task attachment as distinct
   outcomes.

## Resource invariants

- Resolve an exact project before creating or moving a Task. Honor an explicit
  project and ask when more than one project plausibly matches.
- Keep a subtask in its parent's project unless the user requests a valid tree
  move.
- Verify every create or move by reading the resulting Task and checking its
  project, parent, and requested fields.
- Keep Session delivery, standalone Storage, managed files, and Task
  attachments distinct. Uploading or mentioning a file ID does not prove it
  is attached.
- Verify a durable Task attachment by reading the exact Task's attachment
  relation. Reuse a compatible existing file instead of uploading duplicates.
- Report delivery, upload, and attachment failures separately.

## Planning and record coordination

- Record each repository-specific pull request on the Task.
- Record material planning changes, handoffs, blockers, pull requests, and
  completion evidence in the Task timeline.
- Inspect planning dependencies, dependents, related Tasks, relevant comments,
  and real attachments before acting on an existing Task.
- Use a dependency only for a real prerequisite and a related edge for
  non-blocking coordination.
- Do not infer execution state, approval, or completion from a Task, Inbox
  entry, review Gate, Storage file, or Session-delivery view.
