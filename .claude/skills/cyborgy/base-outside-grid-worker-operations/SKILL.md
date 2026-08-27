---
name: base-outside-grid-worker-operations
description: Use only for explicitly authorized worker lifecycle operations launched by an independent operator outside the Cyborgy Workers grid. Never use this skill to weaken the runtime safety rules of an agent running inside a worker session.
---

# Base Outside-Grid Worker Operations

This procedure applies only outside the Workers grid.

- Change lifecycle state only when the user explicitly requested it or granted standing permission for that task.
- Verify through the control plane that the target worker has no active sessions; use a managed drain or handoff path when needed.
- Preserve the intended launch command and root, operate on one positively identified PID, relaunch detached, and verify registration.
- An in-grid agent must follow its runtime system safety rules instead of this skill.
