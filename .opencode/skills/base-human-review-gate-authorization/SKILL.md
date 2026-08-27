---
name: base-human-review-gate-authorization
description: Authorize Human Review Gate mutations only after an explicit user request, and enforce the no-unsolicited-Gate rule across planning records, Skills, and handoffs.
---

# Human Review Gate Authorization

Human Review Gates are user-facing planning and record state. Creating or updating one is an externally visible mutation and requires an explicit request from the user for that operation.

## Authorization rule

- A user must explicitly request the Gate creation or Gate update/revision. Record the requested operation and target before calling it.
- An active Task, durable design or PDF, an agent judgment that approval would be useful, an existing convention, or submitting a revision does not grant authorization.
- Without explicit user authorization, do not create a Gate, update a Gate, or change its revision. Leave the Task Tree unchanged and report the available artifact or ask the user if they want a Gate.

## Safe operation

When the user has explicitly authorized the operation, use the authenticated-owner review handoff. Preserve the stable Gate identity, source Task, source project/parent, revision monotonicity, exact pending/done state machine, and Inbox record. Never accept a raw user ID, email, project, parent, or review decision from the caller. Never decide that the user approved an artifact or grant source Task/Session authority.

If a source Task is cancelled or deleted, reject a new review request. A completed source may still be reviewed only when the existing product state and explicit user request permit it. A pending Gate must not be silently deleted or recreated as a side effect of source deletion.

## Relationship to user-action handoff

`base-user-action-task-handoff` routes a concrete human-only action through the authenticated owner's Action Inbox. It must not create an implicit review Gate. `base-user-review-handoff` owns the authenticated review request and verification flow, but this authorization rule remains the prerequisite for every Gate mutation.
