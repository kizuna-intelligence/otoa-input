---
name: base-worker-waits
description: Use when waiting for a known Cyborgy worker, agent session, background job, or pull-request review, and when deciding whether a Workers-grid session may hand off or an outside-grid agent must continue waiting. Prevents rapid polling and defines bounded idle, offline, timeout, durable review handoff, and review-response handling.
---

# Base Worker And Review Waits

Use this skill whenever progress depends on another worker or review.

- Do not repeatedly enumerate all sessions merely to wait.
- For a known worker, use a bounded worker-idle wait when available. Idle means no active sessions; offline is not safe idle; timeout means still busy.
- After timeout, wait normally or repeat the bounded wait at a sensible interval; never busy-loop.
- Inside the Cyborgy Workers grid, a pending worker, job, or review is not by
  itself a user-waiting state. Use the bounded wait mechanism, continue
  independent work, and report understandable progress. Do not report
  `user_waiting` unless an immediate user answer or action is required before
  the next meaningful step.
- Outside the Workers grid, continue waiting for the PR review as long as the execution environment permits. Wait a reasonable interval, inspect the PR result directly, and act immediately when review resolves: address blocking findings and resubmit review, or follow repository merge policy when review and checks pass.
- If an outside-grid execution limit, unavailable API, failed review session, or other concrete blocker prevents continued waiting, report that exact state; do not call the review complete.
- Track detached jobs through their stable log and status files.
