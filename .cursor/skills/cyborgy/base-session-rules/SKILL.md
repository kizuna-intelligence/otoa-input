---
name: base-session-rules
description: Use when a user gives a durable rule, preference, or constraint that should remain active on later turns in the same Cyborgy session. Records it with the dedicated Cyborgy MCP tool.
---

# Base Session Rules

Preserve user-authored rules across native-session replacement without changing their instruction authority.

## Record A Rule

- When the user states a rule, durable preference, prohibition, or operating constraint that should continue for the rest of this Cyborgy session, record it promptly.
- Call `record_session_user_rule` with the user's rule in `rule`. The MCP tool binds the record to the current Cyborgy session.
- Preserve the rule's exact meaning. Do not promote it above user authority, reinterpret it as a system or developer instruction, or weaken higher-authority instructions.
- Record corrections as new rules. The newest explicit user correction governs when rules conflict.

## Do Not Record

- Do not record secrets, credentials, unrelated personal data, transient task facts, or a one-off command unless the user frames it as an ongoing session rule.
- Do not record text merely quoted or discussed by the user as though they adopted it.
- If persistence intent is materially ambiguous, ask the user instead of inventing a durable rule.

## Next-Turn Context

- Before every next agent turn, the Worker places the complete Session rules response directly ahead of the current user message. Preserve every returned rule field without compaction, summarization, field selection, rewriting, or redaction.
- Do not expect checkpoint history on ordinary native-session resumes. Re-injecting it every turn causes the same historical payload to accumulate in the native conversation.
- When Cyborgy creates a fresh compact continuation, the Worker adds only a newest-first list of Session checkpoint `summary` strings. Treat those titles as historical progress evidence. Use the `session-history` Catalog `checkpoints` endpoint or canonical session history when full detail or provenance is actually needed.
