---
name: base-work-completion
description: Report the current Session's progress or completion in clear user-facing language, choose complete versus user_waiting only from whether the user's work is finished or needs an immediate user action, and, when requested, send an agent-authored ordinary input to an explicitly named Session.
---

# Progress And Completion

Call `display_work_progress_to_user` for intermediate milestones as described by
`base-mcp-basic-usage`. Call `notify_work_complete` for the current Session tile
and its canonical completion checkpoint. Session notifications do not mutate
planning Tasks; when a Task record must change, use the Task Manager explicitly
under `base-task-defaults`.

## User-facing wording

- Write plans, progress, waiting requests, and completion reports in the user's
  language. For a Japanese-speaking user, write natural, straightforward
  Japanese.
- Lead with what was accomplished, what changed for the user, what remains, and
  the next action. Do not lead with Task IDs, Session IDs, API IDs, revision
  names, field names, enum values, or implementation-specific terminology.
- Translate necessary technical detail into ordinary language. Include an exact
  identifier or internal term only when it helps the user locate, verify, or
  troubleshoot something, and place it after the plain-language explanation.
- Make the one-line summary understandable without the detailed summary.
  Internal identifiers alone are not a valid summary.

## Completion state

- Use `complete` only when all work the user asked for is actually finished and
  no required work remains.
- Use `user_waiting` only when the requested work is unfinished and the agent
  cannot make the next meaningful step without an immediate answer, permission,
  credential, other action, or a decision that cannot be taken provisionally,
  from the user. State the exact question or action in plain language.
- Do not use `user_waiting` merely because another worker, background job, PR
  review, external service, or non-blocking user action is pending.
- If the agent can continue useful work without an immediate user response,
  keep working. When a real human-only action will be needed later, create and
  assign an authenticated user-action Task through
  `base-user-action-task-handoff`, then continue all independent work.
- When the missing input is a user decision rather than a human-only action,
  take a provisional value and keep working per `base-provisional-decisions`
  instead of reporting `user_waiting`.
- Do not use `complete` merely because the current turn is ending, a partial
  milestone was reached, or a follow-up Task was recorded. `complete` is still
  correct when the deliverable is finished under provisional values that the
  report states, together with their pending confirmation Tasks.

When a requester needs the work outcome, the agent decides what to report and
sends it as ordinary prompt text through `send_input_to_agent_session` using
the exact requester Session ID stated in the assignment. The system must not
infer recipients, aggregate results, create result envelopes, or provide a
result-specific delivery lane. Do not pass WorkRun, WorkEdge, result, review,
delivery, rollout, or provenance fields to generic Session input.

Work Graph access is relation-only: one Worker has one Graph, every Session on
that Worker is a node, and edges describe only explicit Session relationships.
