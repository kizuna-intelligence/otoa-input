---
name: cyborgy-ml-experiment-tracking
description: "絆のML実験管理として、runs、configurations、metric histories、models、checkpoints、datasets、summaries、comparisonsをRDB-backed Cyborgy Tables、Organization指定Storage、Hugging Faceで管理します。Use for ML実験, machine-learning experiment tracking, training runs, time-series metrics, hyperparameter comparison, or reproducible model evaluation. Do not use Firestore Tables, raw SQL, or the retired Experiment Tracker API."
---

# 絆 ML実験管理

Keep experiment metadata in private RDB-backed Cyborgy Tables. Store model weights,
training checkpoints, used datasets, and other data artifacts in approved
Hugging Face repositories only when the user explicitly requests their upload.
Store complete time-series metric CSV snapshots separately in the
Organization-designated Cyborgy Storage destination through the official
`storage` API.

## Resolve The Organization Destinations

1. Read the current Task and the applicable Organization and repository rules
   for the exact Organization, Hugging Face repo IDs, and Storage root.
2. Treat the Hugging Face and Storage destinations as separate choices. Never
   derive either destination from a Git remote, repository name, user identity,
   folder name, or a similarly named Organization.
3. If the exact Organization or either destination remains unknown or
   ambiguous, ask the user before creating a repository, directory, or upload.
   Never select a plausible Organization as a fallback.
4. Confirm the resolved destinations are writable and satisfy their privacy
   rules before starting a run. Do not change repo visibility or Storage
   ownership without explicit authorization.
5. Treat a request to run, track, compare, or summarize an experiment as
   insufficient authority to upload models, checkpoints, datasets, database
   exports, or other data artifacts to Hugging Face. Upload them only when the
   user explicitly requests the Hugging Face upload.

## Resolve The Table Capability

1. Discover the official `tables` API with `search_apis`.
2. Read its current endpoint schemas before the first call.
3. Use the RDB-specific `tables` group only. Never discover or call the
   separate `firestore-tables` group for ML experiment metadata.
4. Use `call_api("tables", inputs, endpoint_id)` for every metadata operation.
5. Never call `get_sql_namespace`, `execute_sql`, a raw database endpoint, or
   the retired `experiment-tracker`.

## Initialize The Tables

List tables and reuse exact names before creating anything. Create missing
tables separately with the `create` endpoint and these logical schemas:

```json
{"columns":[{"name":"id","type":"string","nullable":false},{"name":"name","type":"string","nullable":false},{"name":"description","type":"string","nullable":false,"default":""},{"name":"repository","type":"string","nullable":false},{"name":"archived","type":"boolean","nullable":false,"default":false},{"name":"created_at","type":"timestamp","nullable":false},{"name":"updated_at","type":"timestamp","nullable":false}],"primary_key":["id"]}
```

Name this table `ml_experiment_projects`.

```json
{"columns":[{"name":"id","type":"string","nullable":false},{"name":"project_id","type":"string","nullable":false},{"name":"run_key","type":"string","nullable":false},{"name":"display_name","type":"string","nullable":false,"default":""},{"name":"status","type":"string","nullable":false,"default":"running"},{"name":"config","type":"json","nullable":false,"default":{}},{"name":"summary","type":"json","nullable":false,"default":{}},{"name":"tags","type":"json","nullable":false,"default":[]},{"name":"notes","type":"string","nullable":false,"default":""},{"name":"git_commit","type":"string","nullable":false,"default":""},{"name":"task_id","type":"string","nullable":false,"default":""},{"name":"session_id","type":"string","nullable":false,"default":""},{"name":"work_run_id","type":"string","nullable":false,"default":""},{"name":"parent_run_id","type":"string","nullable":true},{"name":"started_at","type":"timestamp","nullable":false},{"name":"finished_at","type":"timestamp","nullable":true},{"name":"updated_at","type":"timestamp","nullable":false}],"primary_key":["id"]}
```

Name this table `ml_experiment_runs`.

```json
{"columns":[{"name":"id","type":"string","nullable":false},{"name":"run_id","type":"string","nullable":false},{"name":"kind","type":"string","nullable":false},{"name":"hf_repo_id","type":"string","nullable":false},{"name":"hf_repo_type","type":"string","nullable":false},{"name":"hf_revision","type":"string","nullable":false},{"name":"path","type":"string","nullable":false},{"name":"sha256","type":"string","nullable":false},{"name":"metadata","type":"json","nullable":false,"default":{}},{"name":"created_at","type":"timestamp","nullable":false}],"primary_key":["id"]}
```

Name this table `ml_experiment_artifacts`. Keep the returned opaque table IDs
for later endpoint calls. Use this table for explicitly requested Hugging Face
artifact uploads only. Preserve the shipped Hugging-Face-only schema and never
attempt schema migration.

Name this table `ml_experiment_storage_artifacts`. Keep the returned opaque table
IDs for later endpoint calls. Use this table for `metrics_csv` rows from
Organization-designated Storage uploads and for exact object metadata lookup.
`storage_path` and `storage_etag` are required metadata in Storage records.

```json
{"columns":[{"name":"id","type":"string","nullable":false},{"name":"run_id","type":"string","nullable":false},{"name":"kind","type":"string","nullable":false},{"name":"storage_object_id","type":"string","nullable":true},{"name":"storage_path","type":"string","nullable":false},{"name":"storage_etag","type":"string","nullable":false},{"name":"sha256","type":"string","nullable":false},{"name":"metadata","type":"json","nullable":false,"default":{}},{"name":"created_at","type":"timestamp","nullable":false}],"primary_key":["id"]}
```
`storage_object_id` is optional because the current Storage API does not return
an object or file ID. Never invent one; record it only when a future response
explicitly provides it.

## Prepare The Artifact Backends Before A Run

1. Read the repository's `AGENTS.md`, README, experiment configuration, and
   other explicit rules for exact Hugging Face model and dataset repo IDs,
   Storage root, paths, and privacy requirements.
2. Apply the Organization destination resolution above before any upload.
3. Discover the official `storage` API with `search_apis` and read the current
   `create_directory`, `upload`, and `list` endpoint schemas.
4. Only when the user requested a Hugging Face upload, confirm `HF_TOKEN` is
   non-empty without printing it. Never pass its value on the command line or
   store it in a table, file, report, or log.
5. Use the environment-provided `hf` CLI for an explicitly requested Hugging
   Face upload only. Confirm every required artifact backend is writable before
   training.

## Start Every Run With Metric Capture

Before training begins:

1. Generate opaque stable IDs such as `mlproj_<uuid>` and `mlrun_<uuid>`.
2. Insert the project and run metadata with the Table `insert-rows` endpoint.
   Check `(project_id, run_key)` with `get-rows` first and resume the exact run
   rather than silently creating a duplicate.
3. Create an untracked run directory allowed by repository rules, normally
   `.cyborgy/ml-experiments/<run_id>/`.
4. Create `metrics.csv` with the exact header:
   `run_id,metric_name,step,value,recorded_at`.
5. Configure the training/evaluation callback before starting work so every
   scalar metric observation is appended immediately with an integer step and
   UTC RFC3339 timestamp. Never keep only the final value.
6. Record enough local state to resume upload after interruption, but never
   write `HF_TOKEN`.

## Flush Time-Series Metrics

- Append one CSV row for every observed training, validation, evaluation, and
  system metric selected by the experiment.
- Flush the local file after each observation. Do not upload once per metric
  step.
- Upload a complete CSV snapshot at each durable training checkpoint and on
  finished, failed, or cancelled termination.
- Use `call_api("storage", ..., "upload")` with the complete local CSV and an
  Organization-approved, checkpoint-specific path below its exact Storage
  root. Do not upload metrics CSV files to Hugging Face.
- Read or list the uploaded object and record its exact Storage path, ETag,
  SHA-256, `kind=metrics_csv`, and an object ID only when returned, in
  `ml_experiment_storage_artifacts`.
- If upload fails, retain the local CSV, mark the run incomplete or failed, and
  report the exact resumable path. Never report the run as durably recorded.

## Store Models, Checkpoints, And Datasets

- Do not upload a model, checkpoint, dataset, database export, or other data
  artifact to Hugging Face unless the user explicitly requests that upload.
  Organization or repository destination configuration identifies where an
  authorized upload goes; it does not itself authorize an upload.
- Upload trained model outputs and checkpoints to the exact repository-approved
  Hugging Face model repo only when explicitly requested.
- Upload or reference every dataset actually used by the run in the exact
  repository-approved Hugging Face dataset repo only when explicitly requested.
- Record immutable Hugging Face commit revisions, not only mutable branch
  names. Include file SHA-256 values.
- Never upload secrets, credentials, private unrelated files, raw user data
  without authorization, or ignored files merely because they share a folder.
- Record each model, checkpoint, and dataset in `ml_experiment_artifacts`.
- Record each metric CSV in `ml_experiment_storage_artifacts`.
- Do not place artifact bytes in Tables.

## Write Safely

- Permit only `running`, `finished`, `failed`, or `cancelled` status.
- Refuse secrets in config, summary, notes, tags, artifact metadata, and CSV.
- Use `expected_version` when updating a row already read by the agent. On
  conflict, reread and reconcile instead of overwriting blindly.
- Finish a run by updating status, finished time, summary, and updated time in
  one `update-row` call after required Storage uploads and any explicitly
  requested Hugging Face uploads succeed.

## Read And Compare

- List runs with bounded `get-rows` calls and explicit project IDs.
- Resolve Hugging Face artifact rows from `ml_experiment_artifacts` to immutable
  revisions and Storage rows from `ml_experiment_storage_artifacts` to exact
  paths and ETags before downloading. Use an object ID only when the stored row
  has one.
- Compare metric histories from the selected CSV artifacts. Bound returned
  samples and summarize min, max, mean, last, and best step when relevant.
- Verify table mutations with a bounded `get-rows` call.

Report project/run IDs, Table IDs, the resolved Organization, Hugging Face repo
IDs and revisions, Storage paths and ETags, any returned Storage object IDs,
metric coverage, and any local CSV still awaiting upload.
