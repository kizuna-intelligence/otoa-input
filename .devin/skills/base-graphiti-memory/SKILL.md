---
name: base-graphiti-memory
description: "Official Cyborgy memory skill for writing and querying Graphiti-style episodes, entities, facts, validity windows, and provenance without storing secrets."
---

# Base Graphiti Memory

Use this skill when work would benefit from durable memory across sessions or repositories.

## Model

Base Graphiti Memory follows the Graphiti/Zep temporal context graph pattern:

- Episodes: raw observations, conversations, documents, issues, or task notes. Treat these as provenance.
- Entities: people, projects, repositories, services, APIs, decisions, constraints, and concepts.
- Facts: subject-predicate-object edges with a validity window. Facts can be current, historical, or superseded.
- Scopes: use global for user-wide preferences and durable facts; use repository with owner/repo for project-specific facts.

## Write Guidance

Write memory when the information is likely to matter later:

- Stable user preferences, required workflow rules, credentials location notes without secret values, deployment conventions, and repo architecture.
- Repository-specific decisions, hidden constraints, review setup, test commands, API behavior, and known operational caveats.
- Explicit changes in state: when a previous fact is no longer true, add a new fact with the new valid state and preserve provenance in the episode text.

Do not store secrets, private keys, access tokens, passwords, or raw credentials. Store only where they are configured or how to request them safely.

## API Usage

Use the Graphiti Memory API when available:

- Index/summary: call API `graphiti-memory-index` through `call_api` with scope, repository, q, limit, and optional as_of. Use this before acting when dynamic memory may affect the task because static skill text cannot contain the latest memory.
- Add: POST /graphiti/memory with scope, repository, text, entities, and facts.
- Search/list graph: GET /graphiti/memory/graph with q, scope, repository, limit, and optional as_of.

When the user explicitly asks for InfiniMemory, use the InfiniMemory-named API aliases:

- Add/search/list: call API `infinimemory-memory` with action `add`, `graph`, or `list`.
- Index/summary: call API `infinimemory-memory-index`.
- Current note: call API `infinimemory-current-note`.

Read global memory for user-wide preferences, credentials-location notes without secret values, deployment conventions, shared workflow rules, and cross-repository decisions. Read repository memory for repo architecture, test commands, review setup, known caveats, and repo-specific decisions. When both may matter, check both.

For repository memory, set repository to owner/repo whenever it is known. For local-only work before a remote exists, write the repository or project directory name in the episode text and prefer repository scope once owner/repo is known.
