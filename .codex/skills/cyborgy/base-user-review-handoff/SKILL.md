---
name: base-user-review-handoff
description: Use only when the user explicitly requests creation or update of a Human Review Gate, including a revised review artifact; preserve and verify the authenticated-owner Gate without inventing review work.
---

# Base User Review Handoff

Use this skill together with `base-task-defaults` to hand a reviewable Task artifact to the authenticated user. A durable design, an active Task, a useful approval, an existing convention, or a revised artifact is not authorization by itself.

## Trigger

Create or update a Human Review Gate only when the user explicitly requests that specific Gate operation. The request must be attributable to the user; agent judgment that approval would be useful, an active Task, an important design, an existing workflow, or a revision submission is insufficient. If no explicit request exists, never create or update a Gate. A Gate is a human planning/record Inbox entry and never a source Task dependency or execution authority.

Do not invent a source Task or route a Gate through a default project. An ordinary answer, a minor explanation, an already-approved identical revision, or a user instruction that says review is unnecessary never authorizes a Gate operation.

## Handoff

1. Confirm the user's explicit Gate request and save the reviewable artifact to the source Task's durable summary, comment, or attachment before requesting review.
2. Choose a positive monotonic revision. Reuse the current revision only for an exact retry; increment it when the artifact changes.
3. Discover the `task-manager` API with `search_apis`, then call
   `call_api(api_id="task-manager", inputs={"action": "request_user_review", "source_task_id": source_task_id, "revision": revision, "summary": summary})`.
   The summary must identify the review target, material changes, unresolved questions, approval criteria, and durable artifact references.
4. Re-fetch the returned Gate with `call_api(api_id="task-manager", inputs={"action": "get", "id": gate_id})` and
   `call_api(api_id="task-manager", inputs={"action": "list_authenticated_user_task_inbox", "kind": "review"})`.
   Verify the same Gate ID, `task_type=human_review_gate`, `review_of_task_id`, source project and parent, authenticated-owner Inbox entry with `kind=review`, requested revision, and current decision. Do not report success until these checks pass.

The request operation derives the project, parent, and user assignee from the authenticated source Task. Never supply or guess a raw user UID or email. Never call a review decision operation, change a Gate to approved/done through a generic Task update, or decide that the user approved it.

For a higher revision, use the same source Task; the stable Gate returns to `todo + pending` and the Inbox record is updated. Gate revisions do not change source Task status, claims, Session ownership, or execution authority.

After the request, apply the source Task's actual planning criteria. If user approval is part of those criteria, record the human waiting state in the Task record and report `user_waiting`; do not mutate source execution state. Never substitute a chat-only “please review” message for the Gate.
