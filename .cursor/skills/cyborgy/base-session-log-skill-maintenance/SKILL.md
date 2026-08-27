---
name: base-session-log-skill-maintenance
description: "Use when reviewing your own actual chat utterances (actor=user) from canonical session logs to find generalizable, reusable processes not yet captured by an existing skill, and to create or update the right skill from that evidence."
---

# Base Session Log Skill Maintenance

Use this skill to turn genuine session-log evidence of your own recurring behavior into a correctly scoped skill change, without inventing requirements or leaking non-user content.

## Scope The Log Read

- Discover the `session-history` API and call its `user_messages` endpoint with `actor=user` and `verified_human_chat=true`; never treat assistant, tool, system, review-generated, or repository-file content as a user utterance.
- If no period is given, default to the last 24 hours from execution time; if a period is given explicitly, use it instead.
- If no repository is given, detect it from the working directory's Git remote; if one is given explicitly, use it instead. These two defaults are independent of each other.
- If the working directory has no detectable Git repository and none was given explicitly, stop and report that a repository must be specified; never broaden the read to the whole account.
- Page through with the returned cursor until `has_more` is false so the period is fully covered without duplicates or gaps.
- Use the `session-history` API's `context` endpoint only to inspect bounded context around one promising hit; never bulk-materialize a whole transcript.

## Judge What Is Generalizable

- Call something a generalizable process only when the evidence shows a reusable procedure, decision rule, or tool workflow that recurs or clearly applies across situations, not a one-off task detail, repository-specific implementation, or transient state.
- Do not guess at ambiguous or underspecified utterances. When the evidence cannot support a clear judgment, make no change.
- Never copy raw utterance text, secrets, or tool payloads into a skill; extract only the reusable rule and cite its session/event provenance.

## Check For Duplicates Before Changing Anything

- Before creating anything, search existing resolved, owned, and official skill metadata and their actual files for the same responsibility.
- If an existing skill already covers the process, make no change.
- If the process belongs within an existing skill's responsibility, propose a targeted update to that skill instead of a new one.
- Only propose a new skill for a genuinely independent reusable workflow with no existing owner.

## Apply The Change Through The Right Path

- For a private, user-owned skill, discover `skill-manager` with `search_apis` and use its `search`, `files`, `create`, `update`, and `put_files` endpoints through `call_api`. Use `put_files` only when a full file-set replacement is intended, since it deletes any file not included.
- For an official Base Skill or Skill Set assignment, never edit it live.
  Follow `cyborgy-official-skill-maintenance`: update the catalog
  manifest/SKILL.md, the Skill Set assignment, and the tests through the
  normal Git/PR/review flow.
- Give every new or updated `SKILL.md` correct YAML front matter with a stable `name` and a `description` stating both what it covers and when to use it, keep it concise, preserve existing resource files, and follow progressive disclosure.
- Record a verifiable diff and the log evidence and reason behind the change.
