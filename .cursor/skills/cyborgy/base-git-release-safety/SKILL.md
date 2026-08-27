---
name: base-git-release-safety
description: Use when completing implementation through commit, branch push, pull-request creation or update, review submission, and gate-passing merge, and before publish, deploy, or release-status reporting. Separates the ordinary development and merge path from separately authorized release, infrastructure, and worker-lifecycle actions while enforcing current-base, identity, evidence, credential, and coordination checks.
---

# Base Git And Release Safety

- Follow the active repository's branch, PR, review, release, and deployment policies; do not import another repository's policy.
- For an ordinary code-change request, follow repository policy through a scoped commit and, when pull requests are the required delivery path, a task-branch push, pull-request creation or update, and review submission unless the user explicitly asks to leave the work local or uncommitted.
- Keep merge authority distinct from release authority. An ordinary code-change request authorizes merging its pull request once the repository-required review approval, required checks, and clean mergeability are verified; do not ask for separate user permission at that point unless the user explicitly asked to keep the pull request open or the repository policy requires user action. Publishing, deploying, infrastructure mutation, and worker lifecycle changes remain separately authorized actions.
- Fetch the base branch before commit, push, merge, or deploy; inspect staleness, integrate deliberately, and rerun affected verification.
- Never expose credentials. Use the repository-approved non-interactive identity and report authorization failure instead of silently switching identities.
- Verify review decision, mergeability, required checks, and merge evidence before reporting a PR ready or merged.
- Coordinate conflict-prone pushes, deployments, infrastructure changes, and shared-resource use before acting.

## Official Skill-Only Release

- Use `scripts/deploy-skills.sh dev|prod` when the authorized change is limited
  to selected official Skills and, optionally, the canonical Base Skill Set.
  This path must not deploy Backend, Web, APIs, Apps, Cloud Run, Organization
  Skill Sets, or Repository Skill Sets.
- Pass every exact catalog Skill ID with a repeated `-skill-id`. Run without
  `-apply` first and confirm that only the intended Skill manifests appear.
- Add `-assign-base` only when every selected Skill is broadly applicable to
  all Cyborgy sessions. It appends missing official public Skills to the one
  live Base Skill Set and preserves its existing assignments. Do not use it
  for Organization- or Repository-specific Skills.
- After the dry-run, rerun the same command with `-apply`. Environment
  authorization, a clean checkout at the exact latest `origin/main`, the
  canonical named gcloud configuration, and its approved service account
  remain mandatory.
- Assign an Organization or Repository Skill Set separately through its
  existing canonical API after the Skill is published. Read the current direct
  Skill IDs immediately before calling a replacement-style setting API and
  preserve every existing ID that the user did not ask to remove.
- Treat a partial failure as safely retryable: selected Skill publication and
  Base assignment are idempotent, but they are not one cross-store
  transaction. Verify the published Skill Git HEAD and the final Skill Set
  assignments before reporting completion.
