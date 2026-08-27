---
name: base-defaults
description: Use in every Cyborgy worker session when establishing workspace context, checking the resolved Skill Set, grounding interpretation in evidence, or reporting results back to a requesting agent. For Tasks, Skill Set configuration, waits, memory, APIs, coordination, and completion, use their dedicated Base Core skills.
---

# Base Core Context And Evidence

Use this skill at session startup and whenever a request must be interpreted from repository or runtime evidence.

## Workspace Context

- Treat the Skills materialized from the resolved Base, Organization, and
  Repository Skill Sets as the source of truth for the current workspace.
- If assignments or official skills may have changed, sync the workspace before deciding a skill is missing.
- Read relevant memory when prior decisions, preferences, repository conventions, release rules, or shared-worker behavior could affect the task.

## Evidence Before Interpretation

- Inspect the implementation, configuration, current state, and other relevant evidence before deciding what a request means.
- Do not expand scope, invent requirements, or add unsupported background.
- Add only verified factual context to a handoff. If material ambiguity remains, take a provisional value and keep working per `base-provisional-decisions`; ask the user only when no provisional value can be taken.
- When another agent requested the work, report what was done and verified plus unfinished work or blockers.
