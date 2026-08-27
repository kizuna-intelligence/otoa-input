---
name: base-integration-testing
description: Use when implementing or reviewing changes that affect test coverage, multiple components, protocols, authentication, persistence, deployment, shared fixtures, or user-visible flows; when classifying functional and integration tests; when checking test coverage for regression; or when selecting, launching, and directing an independent QA Session through the Workers UI for cross-boundary verification.
---

# Base Test Classification And Impact Analysis

Classify tests by the behavior they prove, not their filename or directory.

## Test Types

- A **functional test** exercises one bounded feature through its meaningful interface while interacting with the required execution environment. It may use local processes, files, databases, browsers, services, or deterministic test doubles, but it does not claim to prove a complete user or system journey.
- An **integration test** is an end-to-end test for the claimed flow. It must traverse the complete behavior path from the real entry point, across the relevant component boundaries, through the observable result or downstream effect.
- Do not label a partial component path, isolated adapter check, or mocked single-feature check as an integration test. Classify it as a functional test or a narrower test instead.
- When adding a new integration test, make it end-to-end. If the full flow cannot be exercised, add the strongest honest functional coverage and record the missing boundary; do not weaken the definition of integration test.

## Impact And Non-Regression

- Inspect the final implementation and test diff, then trace direct and downstream behavior across relevant application, protocol, authentication, persistence, deployment, and user-visible surfaces.
- Verify that existing functional and integration coverage still proves the supported behavior. Do not accept removed assertions, disabled cases, narrower fixtures, reduced matrices, skipped flows, or relabeled tests that silently reduce coverage or functionality.
- Treat a functional or integration test regression as blocking unless the user explicitly requested the corresponding feature or support reduction. When reduction was explicit, verify that the removed behavior, tests, and documentation match exactly that authorized scope.
- Follow the repository's test commands and suite boundaries. Run every affected flow; use focused suites for bounded impact and the full suite for shared or cross-cutting changes.
- Add or extend the correctly classified test when changed behavior lacks coverage.
- Record the impact analysis, classification, exact commands, results, and concrete reasons for omitted related suites.

## UI Integration

- For a UI flow, send the instruction, perform the operations, and observe the result through the UI. API-only coverage is functional coverage, not an end-to-end UI integration test.
- For independent QA of an environment, run the QA controller on a trusted Worker outside the environment under test. The controller uses the target environment's Workers UI to create fresh subject Sessions, enters the test instruction, and observes their state and output externally. A subject Session inside the Worker under test never judges its own QA result.
- Treat Session APIs as UI implementation details. Do not call them to replace the UI, reuse an existing QA Session, start a Worker from a repository script, or alter the host Worker.
- For a GitHub pull-request flow, create and push a real branch, create the real pull request, start a separate normal reviewer Session with `start_agent_session`, send its report to the exact requester Session with ordinary input, then verify the resulting change through the existing Task/Session flow. Pull-request creation or reviewer Session start alone is not end-to-end completion.

## Independent QA Session

The executing agent decides whether the behavioral impact requires an
independent QA Session and records that judgment with the test evidence. Do not add a backend, planner, deployment, or test-runner gate that substitutes a
hard-coded path rule for that judgment.

Use an independent Session when the change crosses boundaries that should not
be verified only by the implementing Session, such as Worker and Session
behavior, agent authentication or profile selection, Task reference metadata, Agent
Settings, GitHub pull-request flows, or deployment and release behavior.

Launch a fresh QA controller Session whenever independent QA is required. For
dev QA, launch it on a production Worker and give it a separate `cyborgy-dev`
MCP profile; do not launch the controller inside the dev Docker Worker being
tested.
The QA controller's execution procedure begins after that controller has been launched and has received the repository, QA scope, commands, and evidence requirements. Do not pass a target SHA to the controller as an input or prerequisite.
Do not include the implementing Developer's request or the QA controller launch operation in the controller's execution steps; those are prerequisites.
The QA controller determines and verifies the repository's correct latest SHA itself, checks out that SHA, confirms cyborgy-dev Backend and Web are that same SHA, Ready, and at 100% traffic, runs the selected tests, creates only the required dev subject Sessions through the target Workers UI, and reports commands, results, omissions, and subject cleanup in its production Workers UI tile.

## Release evidence order and receipt

When a release uses `scripts/dev-release-evidence.py`, the existing QA Task
summary and the controller's instructions are the contract for the receipt.
Before the controller finishes, make the Session `completion_summary` exactly:

```
QA PASSED
source_sha=<exact 40-character SHA>
subject_session_id=<ID>
```

For multiple subjects, repeat the last line in the same order as the
`--qa-subject-session-id` arguments. Do not add prose, blank lines, or a second
completion field. Run `record` while the controller and subjects are still
terminal and readable; it creates the first owner-bound immutable artifact.
After the existing Session cleanup/finalizer has deleted the exact controller
and subjects, run `dev-release-evidence.py cleanup` with the same owner-bound
tokens. It verifies the recorded UID/Worker identity before interpreting a 404,
then appends the deletion receipt to the hash-linked terminal artifact. The
later `verify` consumes only that terminal artifact and live component state;
it does not use a deleted Session's 404 and does not require cleanup API
credentials. Running `verify` on the record artifact fails with the cleanup
command to run next.
