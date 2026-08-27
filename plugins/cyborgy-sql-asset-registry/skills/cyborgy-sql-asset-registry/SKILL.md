---
name: cyborgy-sql-asset-registry
description: "Manages private Dataset and Model assets with immutable versions, aliases, manifests, lineage, RDB-backed Cyborgy Tables metadata, and Hugging Face files. Use for Asset Registry, dataset versioning, model versioning, checkpoints, reproducible file sets, version aliases, comparisons, or parent provenance. Do not use Firestore Tables, raw SQL, or the retired Asset Registry API."
---

# Asset Registry

Keep Dataset and Model registry metadata in private RDB-backed Cyborgy Tables. Store every
dataset byte, model weight, and checkpoint in the exact Hugging Face repository
selected by the current repository's rules.

## Resolve The Table Capability

1. Discover the official `tables` API with `search_apis`.
2. Read its current endpoint schemas before the first call.
3. Use the RDB-specific `tables` group only. Never discover or call the
   separate `firestore-tables` group for registry metadata.
4. Use `call_api("tables", inputs, endpoint_id)` for every metadata operation.
5. Never call `get_sql_namespace`, `execute_sql`, or the retired
   `asset-registry`.

## Initialize The Tables

List tables and reuse exact names before creating anything. Create the following
logical tables when missing:

- `assets`: `id` string primary key; `kind` string; `name` string; `slug`
  string; `description` string; `tags` json; `archived` boolean; `created_at`
  timestamp; `updated_at` timestamp.
- `asset_versions`: `id` string primary key; `asset_id` string;
  `version_number` integer; `state` string; `format` string; `framework`
  string; `schema_fingerprint` string; `reproducibility` json;
  `github_repository` string; `github_commit_sha` string; `created_at`
  timestamp; nullable `ready_at` timestamp.
- `asset_version_files`: `id` string primary key; `version_id` string;
  `hf_repo_id` string; `hf_repo_type` string; `hf_revision` string; `path`
  string; `role` string; `media_type` string; `size_bytes` integer; `sha256`
  string; `metadata` json; `created_at` timestamp.
- `asset_version_parents`: `id` string primary key; `version_id` string;
  `parent_version_id` string.
- `asset_aliases`: `id` string primary key; `asset_id` string; `alias` string;
  `version_id` string; `updated_at` timestamp.

Use the abstract types supported by the current `create` endpoint. Make every
field non-null unless it is explicitly described as nullable. Use empty values
and JSON defaults deliberately rather than inventing backend-specific types.
Keep the returned opaque table IDs for later calls.

## Resolve Hugging Face Repositories

1. Read `AGENTS.md`, README, release or training configuration, and other
   explicit repository rules for exact Hugging Face repo IDs, repo types,
   paths, privacy requirements, and revision policy.
2. Use a model repo for models and checkpoints, and a dataset repo for
   datasets. Follow a more specific repository rule when present.
3. Do not derive a Hugging Face account, organization, or repo ID from a Git
   remote. Ask the user when no exact destination is specified.
4. Confirm `HF_TOKEN` is non-empty without printing it. Never pass its value on
   the command line or save it in Tables, manifests, files, logs, or reports.
5. Use the environment-provided `hf` CLI. Do not create a repository or change
   visibility without explicit authorization.

## Create A Version

1. Generate opaque stable IDs such as `asset_<uuid>`, `assetv_<uuid>`, and
   `assetfile_<uuid>`.
2. Read the selected Asset and existing Versions with bounded `get-rows` calls.
   Choose the next version number and insert a `draft` Version.
3. Upload each dataset, model, checkpoint, and manifest with
   `hf upload <repo_id> <local_path> <path_in_repo> --repo-type model|dataset`.
   Let `HF_TOKEN` flow only through the environment.
4. Record the exact Hugging Face commit revision, repo ID, repo type, logical
   path, SHA-256, size, media type, and role in `asset_version_files`.
5. Add explicit parent Version rows and code provenance.
6. Verify every recorded file at the immutable Hugging Face revision and read
   the complete bounded manifest.
7. Update the Version to `ready` with `ready_at`. Never edit the files, parents,
   or reproducibility metadata of a ready Version; create another Version.
8. Move the `latest` alias only after readiness is verified.

If any upload or metadata step fails, leave the Version as `draft`, retain
local resumable files, report the exact incomplete state, and resume
idempotently.

## Write Safely

- Permit only `dataset` or `model` kinds and `draft`, `ready`, or `archived`
  version states.
- Restrict slugs and aliases to lower-case letters, digits, and hyphens.
- Confirm an alias target belongs to the same Asset and is ready.
- Refuse secrets in descriptions, tags, reproducibility data, code metadata,
  file metadata, paths, and uploaded files.
- Reject absolute logical paths, `..` traversal, duplicate paths, negative
  sizes, and mutable-only revision references.
- Use `expected_version` when changing a row previously read by the agent.
- Archive Assets and Versions instead of deleting them unless the user
  explicitly requests permanent deletion.

## Read, Resolve, And Compare

- Resolve `latest` or another alias, then confirm its Version is ready and
  belongs to the requested Asset.
- Return Version identity, number, Hugging Face file manifest, parent Versions,
  reproducibility metadata, and code provenance together.
- Compare Versions by logical path and report added, removed, and changed files
  using immutable revision, SHA-256, and size.
- Download only selected files from their exact repo and revision.
- Bound list and comparison calls with deterministic ordering and limits.

After every mutation, verify with `get-rows` and report Asset, Version, Alias,
Table, Hugging Face repo, revision, and path IDs affected.
