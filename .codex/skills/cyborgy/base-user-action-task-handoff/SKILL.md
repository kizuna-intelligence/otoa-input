---
name: base-user-action-task-handoff
description: Use when a real human-only action must be routed to the authenticated Task owner through the first-class UserTaskAssignment and Shared Inbox APIs, including non-blocking actions that should be recorded while agent work continues; never use it for agent work or implicit review Gates.
---

# Authenticated User Action Task Handoff

Use this skill with `base-task-defaults` when work genuinely requires the authenticated human to perform an action. This is a user-action handoff, not a generic task assignment and not a Human Review Gate.

## Classification and routing

1. Confirm that the action cannot be performed by the current agent or an authorized Cyborgy executor. Explain the concrete human-only reason in `reason`; do not put secrets or raw user identifiers in it.
2. Use the active source Task's project and, when requested, its existing parent. Set `related_to_task_id` to the source Task. Never accept or invent `user_id`, email, or an assignee list; the server derives the authenticated owner.
3. Record the source relation with `related_to_task_id`. Set `depends_on_task_ids` only when another Task is a true planning prerequisite. Neither relation grants authority or gates execution, and a user action must not be used to smuggle a review Gate into a graph.
4. Decide separately whether the action blocks current work. If useful work can
   continue before the user acts, create the assigned user-action Task and keep
   working. Do not stop or report `user_waiting` merely because the Task exists.
   Use `user_waiting` only when the user's action is required before the next
   meaningful step.

## Create and retry

1. Discover the `task-manager` API with `search_apis`, then call
   `call_api(api_id="task-manager", inputs={"action": "create_authenticated_user_action_task", "name": ..., "summary": ..., "related_to_task_id": ..., "reason": ..., "idempotency_key": ...})`
   with a clear name, actionable summary, source Task relation, bounded reason, and a stable `idempotency_key`.
   Write the name and summary in the user's language and explain the requested
   action and its purpose in ordinary terms. Do not use an ID or internal field
   name as the user-facing title.
2. Treat the create response's Task, owner assignment, unread Inbox entry, and assignment event as one atomic result. On retry, reuse the exact key and payload; a different payload with that key is a conflict.
3. Do not use the ordinary generic Task create path to represent this handoff. Do not expose or submit a raw owner ID.

## Assignment and Inbox

- Use `call_api(api_id="task-manager", inputs={"action": "set_authenticated_user_task_assignment", "id": task_id, "assigned": ..., "expected_task_version": ..., "reason": ..., "idempotency_key": ...})` only for an ordinary non-terminal Task and pass the exact `expected_task_version`, a reason, and a new idempotency key. Assignment changes are owner-scoped and optimistic-concurrency guarded; they change Inbox visibility only.
- Use `call_api(api_id="task-manager", inputs={"action": "list_authenticated_user_task_inbox", "kind": "action"})` to show `kind=action` entries separately from `kind=review`. Read markers (`action=mark_authenticated_user_task_inbox_read`) are durable and only mark the selected entry read; they do not complete the Task or decide a review.
- If terminal status or a version change prevents the operation, re-fetch and reconcile instead of overwriting it.
- In progress or completion reports, refer to the user action by its
  plain-language purpose or title. Include its Task ID only as secondary lookup
  information when useful.

## Review boundary

Never create or update a Human Review Gate unless the user explicitly requests that specific Gate operation. Approval that seems useful, an important design, a revised artifact, or an existing Task convention is not authorization; use `base-user-review-handoff` only after explicit user authorization. A Gate is a human planning/record Inbox entry, not a source Task dependency or execution authority.
