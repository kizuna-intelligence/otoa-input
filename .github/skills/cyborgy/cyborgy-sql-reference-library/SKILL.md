---
name: cyborgy-sql-reference-library
description: "絆の文献管理として、private research-reference library、ユーザーとのdiscussion notes、CSL-JSON、attached papersをRDB-backed Cyborgy TablesとOrganization指定Storageで管理します。Use for 参考文献管理, bibliography projects, DOI or other scholarly identifiers, reading status, project decisions, citation keys, duplicate merges, and attached papers. Do not use Firestore Tables, raw SQL, or the retired Reference Manager API."
---

# 絆 文献管理

Keep bibliographic metadata in private RDB-backed Cyborgy Tables. Keep papers and
supplements in the Organization-designated Cyborgy Storage destination and save
exact Storage paths and ETags, plus opaque IDs only when the Storage API returns
them.

## Resolve The Organization Storage Destination

1. Read the current Task and the applicable Organization and repository rules
   for the exact Organization and Storage root.
2. Never derive the Organization or Storage destination from a Git remote,
   repository name, user identity, folder name, or a similarly named
   Organization.
3. If the exact Organization or Storage destination remains unknown or
   ambiguous, ask the user before creating a directory or uploading a file.
   Never select a plausible Organization as a fallback.
4. Confirm the resolved destination is writable and satisfies its privacy rules
   before uploading.

## Resolve The Table Capability

1. Discover the official `tables` API with `search_apis`.
2. Read its current endpoint schemas before the first call.
3. Use the RDB-specific `tables` group only. Never discover or call the
   separate `firestore-tables` group for reference metadata.
4. Use `call_api("tables", inputs, endpoint_id)` for every metadata operation.
5. Never call `get_sql_namespace`, `execute_sql`, or the retired
   `reference-manager`.

## Initialize The Tables

List tables and reuse exact names before creating anything. Create these logical
tables when missing:

- `reference_projects`: `id` string primary key; `name` string; `description`
  string; `archived` boolean; `created_at` timestamp; `updated_at` timestamp.
- `references`: `id` string primary key; `csl` json; `doi`, `pmid`, `pmcid`,
  `arxiv`, `openalex`, `isbn`, and `abstract` strings; `keywords` json;
  `reading_status` string; nullable `rating` integer; `library_note` string;
  `provenance` json; `archived` boolean; nullable `merged_into_id` string;
  `created_at` timestamp; `updated_at` timestamp.
- `reference_project_memberships`: composite primary key
  `reference_id, project_id`; `decision` string; `tags` json; `citation_key`
  string; `note` string; `created_at` timestamp; `updated_at` timestamp.
- `reference_attachments`: composite primary key
  `reference_id, file_id, role`; all three fields are strings; `created_at`
  timestamp. Preserve this shipped legacy table and use it only when the
  Storage API explicitly returns a `file_id`; never invent one.
- `reference_storage_attachments`: composite primary key
  `reference_id, storage_path, role`; all three fields are strings;
  `storage_etag` string; nullable `storage_object_id` string; `created_at`
  timestamp. Use this table for the current Storage API.
- `reference_merge_audits`: `id` string primary key; `survivor_id` string;
  `duplicate_id` string; `field_choices` json; `snapshot` json; `merged_at`
  timestamp.
- `reference_discussion_notes`: `id` string primary key; `project_id` string;
  nullable `reference_id` string; `session_id` string; `key_points` json;
  `details` string; nullable `storage_object_id`, `storage_path`, and
  `storage_etag` strings; `created_at` timestamp; `updated_at` timestamp.

Use the abstract types supported by the current `create` endpoint. Make every
field non-null unless explicitly nullable, apply sensible empty JSON/string
defaults, and keep the returned opaque table IDs.

## Write Safely

- Generate stable opaque IDs such as `ref_<uuid>`, `refproj_<uuid>`, and
  `refmerge_<uuid>`. Use `refdiscussion_<uuid>` for a new discussion note and
  update the same note while the same discussion continues.
- Store a complete CSL-JSON object in `csl`; preserve unknown CSL fields.
- Normalize DOI by removing `https://doi.org/` or `doi:` and lower-casing it.
  Normalize other identifiers before duplicate checks.
- Before insert, use bounded equality lookups for every supplied canonical
  identifier. Report exact duplicates instead of adding another row.
- Permit only `unread`, `to_read`, `reading`, or `read` reading status; rating
  must be between zero and five.
- Refuse secrets in notes, CSL fields, provenance, tags, and attachment
  metadata.
- Use `expected_version` when changing a row already read by the agent. On
  conflict, reread and reconcile.
- Archive records instead of deleting them unless the user explicitly requests
  permanent deletion.

## Maintain Discussion Notes

1. Create or update a discussion note when the user asks to preserve a
   discussion or when a substantive discussion changes the project's scope,
   interpretation, source selection, or decision. Do not record greetings,
   repetition, or unrelated conversation.
2. Follow the user's instruction about what belongs in the note. Default to a
   short Markdown `要点` list that preserves only what was actually discussed
   and decided. Do not add claims, rationale, or certainty the user did not
   provide.
3. Add a `詳細` section only when the user asks for it or when the key points
   would otherwise lose necessary qualifications, alternatives, disagreement,
   or decision rationale.
4. Upsert `key_points`, optional `details`, `session_id`, and `updated_at` in
   `reference_discussion_notes` after each material update. Use
   `expected_version`; reread and reconcile on conflict.
5. Render the current note to a Markdown file and upload it through the
   official `storage` API when the user requests the file or when the
   applicable Organization workflow requires durable file storage or sharing.
   Otherwise keep the Storage fields null; the Table record is still required.
6. When uploading, apply the Organization destination resolution above, use a
   stable Organization-approved path, verify the uploaded object, and record
   its exact path and ETag on the discussion-note row. Record an object ID only
   when the Storage API returns one; never invent it. Update the record whenever
   a later upload changes the durable file.

## Attach Files

1. Discover `storage`, then upload or find the paper with its `upload` and
   `list` endpoints through `call_api`.
2. Apply the Organization destination resolution above. Record the exact
   Storage path, ETag, and role in `reference_storage_attachments`. Record a
   nullable object ID only when returned.
3. Use the legacy `reference_attachments` table only when Storage explicitly
   returns a `file_id`; never synthesize one from a path or filename.
4. Retrieve or delete bytes through Storage by exact path. Before deletion,
   read both attachment tables and warn when any reference still points to the
   file.
5. Never store base64 file bodies in Tables.

## Search, Cite, And Merge

- Use bounded equality filters for canonical identifiers, project membership,
  decisions, and exact tags supported by the current Table API. Fetch a
  bounded candidate set before doing title, author, abstract, or keyword text
  matching in the agent.
- Fetch the full CSL object for citation or bibliography export. Preserve the
  requested style outside the canonical stored record.
- For a merge, first read both records and all related membership and attachment
  rows. Move non-conflicting relations, record a complete audit snapshot, mark
  the duplicate archived with `merged_into_id`, and update the survivor.
- Table calls are individually atomic but a multi-call merge is not. Stop and
  report exact completed and incomplete IDs on conflict or failure.

After every mutation, verify with bounded `get-rows` calls and report affected
reference, project, discussion-note, Table, and Storage file IDs.
