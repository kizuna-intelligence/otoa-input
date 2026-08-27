---
name: clean-old-code
description: Inventory and safely remove obsolete, duplicate, legacy, dead, or superseded code by comparing similar feature implementations with Git history, live consumers, and tests. Use when the user asks to clean old code, delete a previous implementation, consolidate duplicate features, remove dead paths, or reconcile an older and newer implementation.
---

# Clean Old Code

Treat newer code as the leading candidate for intended behavior, not as proof that older code is safe to remove. Inventory first, obtain approval for exact candidates, then clean and verify.

## 1. Establish Scope

- Inspect repository instructions, worktree state, current branch, and the user-named area.
- Preserve unrelated changes and do not broaden a local cleanup into a repository-wide rewrite.
- Do not edit or delete candidates during discovery.

## 2. Inventory Similar Feature Families

- Search names, routes, commands, handlers, components, configuration keys, feature flags, schemas, imports, registrations, and tests for implementations serving similar purposes.
- Group related implementations into feature families instead of listing isolated files.
- Trace static and dynamic consumers, entry points, generated references, persisted data, public APIs, deployment configuration, and compatibility paths.
- Inspect relevant Git evidence with `git log`, `git log --follow`, `git blame`, merge history, and the commits that introduced or replaced each implementation.
- Compare behavior and tests. Prefer the newer implementation as the likely successor when history supports that interpretation, but explicitly record contrary or missing evidence.
- Distinguish obsolete code from intentional fallbacks, migrations, rollback paths, compatibility shims, staged rollouts, and independently supported variants.

## 3. Report Before Removing

Present a compact candidate table containing:

| Candidate | Likely successor | Git/history evidence | Current consumers | Test evidence | Risk/confidence | Proposed action |
|---|---|---|---|---|---|---|

Include all discovered feature families in scope, including uncertain candidates. State what was searched and any blind spots. Separate candidates into:

1. Strong removal candidates: superseded and unreferenced with supporting history or tests.
2. Conditional candidates: probably old but protected by compatibility, migration, rollout, or uncertain runtime use.
3. Keep candidates: still active or intentionally distinct.

## 4. Require an Approval Checkpoint

- Ask the user to approve the exact strong candidates and any explicitly described follow-up changes before deleting or behaviorally disabling code.
- Do not treat a broad request such as “clean old code” as approval of every candidate found.
- Offer candidate IDs so the user can approve all, select a subset, or request more evidence.
- Continue discovery without approval when useful, but pause destructive cleanup at this checkpoint.

## 5. Clean Approved Candidates

- Reconfirm that the approved paths and symbols have not changed since the inventory.
- Remove the obsolete implementation and its now-unused imports, tests, fixtures, flags, configuration, documentation, and registrations only when each is part of the approved candidate.
- Preserve compatibility or data-migration behavior unless its removal was explicitly approved and proven safe.
- Prefer a focused deletion over redesigning the surviving implementation.

## 6. Verify and Report

- Run focused tests for the surviving feature and removed paths, then broader checks proportional to the affected boundaries.
- Search again for dangling references, duplicate registrations, stale configuration, and orphaned tests.
- Confirm the diff contains only approved cleanup and related mechanical fallout.
- Report removed candidates, preserved candidates, evidence, tests, and remaining uncertainty. Never claim code is dead solely because text search found no references.
