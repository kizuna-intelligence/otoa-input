---
name: base-kizuna-note-usage
description: "Official Cyborgy skill for exact Kizuna Note / Kizuna Memory and 絆ノート / 絆メモリー requests, including note discovery, safe content-preserving updates, provenance ingestion, verification, and protection against substitute memory APIs."
---

# Base Kizuna Note Usage

Use this skill whenever the user says Kizuna Note, Kizuna Memory, 絆ノート, or 絆メモリー, or asks to write, save, search, browse, or update user-owned epistemic notes.

## Product Routing

- Treat Kizuna Note and 絆ノート as exact product names, with Kizuna Memory and 絆メモリー retained as legacy aliases. Use only the official Kizuna Note API for any of these names; never substitute the generic `user-memory` Catalog API, Graphiti Memory, or InfiniMemory.
- Discover Kizuna Note with `search_apis`, read its current schema, and invoke it with `call_api`. Its compatibility-stable API ID remains `kizuna-memory`.

## Notes

- For a request to write into Kizuna Note, inspect existing notes with `notes`, `note`, or a purposeful `search`.
- If a clearly relevant note exists, preserve its existing content and append the requested text with `update_note`.
- Create a note only when no relevant note exists or the user asks for a new one. First call `mention_new_note`, then use the active `offer_id` returned by that call in `create_note`; `create_note` requires an active `offer_id`. If the newly created note needs further changes, use `update_note` only after `create_note` succeeds.
- A user's explicit request to write text into their note authorizes that note edit. Verify the saved note with `note` and report its title and ID.
- The current `update_note` handler expects the note body in `content`. Never replace unrelated existing text.

## Provenance And Epistemic Safety

- A claim records what a source asserted; a belief is knowledge the user adopted in a note. An agent must not decide on its own to promote a claim on the user's behalf.
- By default, use `ingest` only and leave adoption to Kizuna Note's automatic adoption flow. Use `ingest_document` for managed stored documents so provenance is retained.
- A user's explicit request to adopt content or a claim authorizes a manual `adopt`. Inspect the target claim and note, invoke `adopt`, verify the resulting note or belief, and report the result. Without an explicit user request, never invoke manual `adopt`.
- Use `reject` only for an explicit user rejection, `resolve` for an explicit resolution of a contested cluster, and `affirm` for an explicit answer to a prior freshness or confirmation prompt.
- Do not force `flush_now` for ordinary utterances. Use explicit processing only when needed, and keep claim searches purposeful.
- Never store secrets, credentials, private keys, or tokens.
