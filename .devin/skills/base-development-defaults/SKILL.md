---
name: base-development-defaults
description: Implement scoped repository changes, reproduce bugs with functional-test-first coverage, use Codex exec-policy-safe shell cleanup, verify them, and deliver one reviewed pull request.
---

Keep implementation, verification, commit, push, and review in one coherent
change when the Task requires a single PR. Delegated agents report themselves
through ordinary input to an explicitly named Session. The system does not
route, aggregate, or deliver delegated results automatically.

## Solve by deletion before solving by addition

- The best fix removes a concept. Before designing anything, ask: **which
  existing flag, field, state, mode, option, or branch can be deleted so that
  the bug becomes impossible to express?** A fix that ends with fewer concepts
  than it started with is worth more than a correct fix that adds one.
- Reach for these in order: delete a concept; **generalize a concept that is
  already there so it covers the new case too**; merge two concepts into one;
  reorganize existing concepts; and only when none of those work, add one.
- **Generalizing beats adding.** Before writing anything new, look for the
  concept that is already ninety percent of what you need and ask what small
  widening makes it cover this case as well. Widening one concept keeps the
  count flat; adding a neighbour raises it forever.
- **But never widen a concept into a mixture.** A concept that has been
  stretched to cover two unrelated things is worse than two honest concepts:
  its name stops predicting its behaviour, and every caller has to know which
  half it is talking to. The test is whether the widened concept still has one
  sentence that describes all its uses. If describing it requires "and also",
  you have made a mixture — back it out.

### Look wide before you touch a concept

A concept is never local. Before changing one, find **every** place it is
produced, consumed, persisted, displayed, and asserted on, and make the change
consistent across all of them in the same PR. A concept that means one thing in
the backend, another in the CLI, and a third in the UI is the source of the
"these two got out of sync" bugs this section exists to prevent — and a partial
change is how a single concept quietly becomes three.

Hold both ends at once:

- **Wide**: no inconsistency left anywhere the concept appears.
- **Elegant**: the concept count does not go up. Covering more ground must come
  from the concept becoming clearer, not from the concept multiplying.

If you cannot have both, you have not found the right concept yet.
- Two things that can disagree are one concept too many. When a bug is "these
  two got out of sync", the fix is usually to delete one of them and derive it
  from the other, not to add reconciliation between them.
- A special case beside a general case is one concept too many. Fold it in.
- A flag that is always set the same way in practice is already dead; delete it
  rather than documenting it.
- State the concept count in the PR: what was removed, what was added. If the
  net is positive, justify why no deletion was available.

### When you truly must add one

Adding is the last resort, reached only after deletion, merging, and
reorganization have each been tried and shown not to work. When you get there,
the thing you add is held to a much higher standard than the thing you delete,
because it is permanent and everyone after you inherits it.

The concept you add must be **general, not a patch for the case in front of
you**. Test it against all of these before writing a line:

- **Name it without referring to the bug.** If the only honest name mentions
  the specific symptom, the caller, or the release it came from, it is a patch
  wearing a concept's clothes. A real concept is named for what it *is*.
- **Find the second and third caller.** A concept with exactly one use site is
  a branch that has been given a formal-sounding name. If no other place in the
  codebase wants it, you have not found the concept yet — keep looking for the
  deletion.
- **It must make a class of bugs impossible, not this one bug unlikely.**
  Ask what becomes unrepresentable once it exists. If the answer is "nothing,
  but the code now checks for it", that is a check, not a concept.
- **It must compose.** It should combine with the concepts already present
  without needing special rules for the combinations. A concept that requires
  "except when X" clauses at its boundaries is two concepts fused badly.
- **It must have exactly one authority.** If the new thing can disagree with
  something that already exists, you have added the very shape this section
  exists to prevent.
- **It should let you delete something.** The strongest justification for a new
  concept is that introducing it retires two or three older ones. If nothing
  can be removed once it lands, be suspicious of it.

If the concept cannot pass these, the design is not finished. Go back and look
for the deletion again — it is almost always there, and it is almost always in
the part of the code you did not want to touch.

### Failure modes when applying deletion

Before changing a deletion path, name the property the change must preserve and
the exact code or test that verifies it. Apply these failure modes to the
concrete producer, consumer, and recovery path:

- **Deleting the guarantee with the concept.** Do not remove a `record` or
  `cleanup` check merely because the surrounding concepts are contradictory or
  being reorganized. Name the property it protects and identify the exact
  enforcement that remains. In the current QA path,
  `scripts/dev-release-evidence.py:trusted_qa_cleanup` checks the Task cleanup
  receipt and calls `get_session` for each recorded Session, but its 404
  handling is ambiguous as described below; preserving that call alone is not
  proof. If no replacement verifies the property, keep the check.
- **Counterfeit guarantee.** Treat an audit trail as evidence only if its reader
  rejects the tampering classes the trail claims to detect. For the receipt
  chain, `scripts/test_account_rotation.py:ReceiptLedger._discover` validates
  artifact sequence, path authority, each journaled artifact's
  `file.content_sha256`, and the `journal.previous_path` /
  `journal.previous_sha256` link. The
  `scripts/test_test_account_rotation.py:test_immutable_receipt_chain_rejects_terminal_envelope_filename_and_gap_tamper`
  test covers representative envelope, filename, and gap changes. Preserve
  these checks or add equivalent tests for any replacement.
- **Dead-end state.** Apply a return-path test only to a persisted state whose
  purpose is to pause or quarantine work after an abnormality and whose state
  machine or handler defines a legal recovery transition. Name the detected
  abnormality, the exact stopped state, the required recovery evidence, and
  the exact destination: the pre-stop state when re-entry is legal, or the
  explicitly named recovery/ready state. Do not apply this mechanically to a
  normal terminal state: when the transition table or handler intentionally
  declares `error` or `unresolved` terminal and provides no recovery operation,
  test that it remains observable and rejects new work instead of inventing a
  return test. A `fence` is not a returnable state unless the affected state
  machine explicitly persists it as one; identify the state and transition the
  fence protects.
- **404 is not proof of deletion.**
  `backend/internal/workers/session_query.go:GetSession` returns `false` both
  when the Session is absent and when its `UID` differs from the caller;
  `backend/internal/api/worker_projection_handlers.go:getMySession` maps both
  cases to 404. `scripts/dev-release-evidence.py:trusted_qa_cleanup` currently
  catches `status_code == 404` and treats it as success, without obtaining a
  separate deletion or visibility signal. Do not pair 404 with an unspecified
  signal. Use the repository's concrete deletion contract: read the exact
  owner-authorized Session before DELETE and retain its snapshot; require a 204
  response with `X-Cyborgy-Session-Deletion-Revision`,
  `X-Cyborgy-Session-Input-Hint: deleted`, and
  `X-Cyborgy-Session-Deletion-Retention: tombstone-7d`; verify that the deletion
  revision is newer than the snapshot revision, then verify the post-delete 404
  and the canonical deletion refresh. This sequence is exercised by
  `web/tests/integration/workers-final-result-sla.dev.spec.ts:deleteSessionThroughUI`
  and its recorded fields are enforced by
  `scripts/finalize-dev-qa-cycle.py:phase_one` (subject cleanup fields) and
  `scripts/finalize-dev-qa-cycle.py:phase_two` (controller deletion receipt and
  404). If these exact owner-scoped receipt and refresh facts are unavailable,
  treat 404 as ambiguous and fail closed.
- **Search before inventing.** Before inventing a new tamper-detection
  mechanism, search the repository for the same problem already solved.
  `scripts/test_account_rotation.py:ReceiptLedger.publish` writes
  `previous_path` and `previous_sha256` into each artifact's `journal`, and
  `_discover` follows those predecessor links. In contrast,
  `scripts/read-test-account-receipt.py:main` calls `_discover`, hashes the
  returned terminal artifact's raw bytes, and emits `terminal_artifact_sha256`
  as metadata; it does not write that field to `receipt.json`.
  `scripts/test_account_rotation.py:read_active_version` recomputes that
  terminal hash and compares it only when `expected_terminal_sha256` is
  provided; `scripts/read-test-account-password.py:main` provides it through
  `--terminal-sha256`. Describe `previous_*` as predecessor links and
  `terminal_artifact_sha256` as a caller-supplied terminal-byte binding, not as
  one field in the receipt chain. To claim terminal-byte tamper detection at a
  consumer boundary, pass the expected terminal path and hash; without the
  expected hash, that comparison is not performed.

## Reorganize existing concepts before adding new ones

- When a bug is understood, the first question is always whether the existing
  concepts, already in the code, can be arranged so the bug cannot occur. Ask
  that before designing any fix.
- Do not reach for a new flag, field, state, mode, or option as the opening
  move. Every one of them is a permanent branch that every future reader,
  every future state transition, and every future test has to account for, and
  a codebase that answers each bug with one more flag becomes unmaintainable
  regardless of how correct each individual flag was.
- Look first for these shapes: one authority instead of two that can disagree,
  a single path that all callers take instead of the same ritual hand-written
  at every exit, a value carried instead of recomputed, an invariant enforced
  at construction instead of checked at use, a case folded into an existing
  one rather than special-cased beside it.
- A sibling code path that already handles the case correctly is the strongest
  evidence available: match its shape rather than inventing a second one.
- Add a new concept only when reorganization has been tried and named, and the
  new concept expresses something the existing vocabulary genuinely cannot.
  When that happens, say in the pull request which reorganization was
  considered and why it was not enough.
- This applies to state kept at runtime as much as to code. Prefer deriving a
  fact from what is already recorded over storing one more field that can
  drift from the rest.

## Bug-fix workflow

- Before changing production code, design the strongest meaningful functional
  test that exercises the affected feature through its real interface and
  reproduces the reported problem.
- Run that test against the unfixed code and confirm it fails for the expected
  reason. A passing test, unrelated failure, or assertion that does not observe
  the reported behavior is not a valid reproduction.
- Use a unit test instead only when a functional test would clearly add no
  meaningful behavioral evidence or when meaningful functional reproduction is
  disproportionately difficult to implement or operate reliably. Record which
  exception applies and why.
- If the affected behavior is an important product capability, add the
  reproduced scenario to dev manual QA coverage so future validation repeats
  the user-visible flow.
- After reproduction is proven, change the program until the same test passes.
  Do not weaken, skip, or replace the reproducing assertion to make the fix
  appear successful; then run the affected broader suites.
- Use `base-integration-testing` to classify functional and integration
  coverage and to select additional cross-boundary verification.

## Codex exec-policy-safe temporary-file cleanup

- Treat a rejection from Codex's own execution policy as a provider failure,
  not as a Cyborgy command failure. The confirmed rejections in the current
  runtime include `rm -f` style cleanup, reported as `rm -f style commands are
  not permitted. Use a safer approach`; the same message was captured for
  `rm -rf` style cleanup. Do not add other forbidden patterns here unless the
  exact Codex runtime has been checked and the pattern is observed.
- Do not omit legitimate cleanup. Keep the temporary path in a variable,
  check that it exists, and remove it without the force option. For example:

  ```sh
  tmp_file="$(mktemp /tmp/cyborgy-qa-response.XXXXXX)"
  trap 'if [ -e "$tmp_file" ]; then rm -- "$tmp_file"; fi' EXIT
  curl -fsS https://example.invalid/status >"$tmp_file"
  ```

  The cleanup is still attempted when the command before it fails, while the
  `rm -- "$tmp_file"` form does not use either confirmed force-removal style.
  Preserve the real command failure; cleanup is not a reason to turn a failed
  action into success.

- For a temporary directory, track each expected file, remove those files with
  `rm --`, and then remove the now-empty directory with `rmdir --`:

  ```sh
  tmp_dir="$(mktemp -d /tmp/cyborgy-qa.XXXXXX)"
  tmp_file="$tmp_dir/response"
  trap 'if [ -e "$tmp_file" ]; then rm -- "$tmp_file"; fi
        if [ -d "$tmp_dir" ]; then rmdir -- "$tmp_dir"; fi' EXIT
  curl -fsS https://example.invalid/status >"$tmp_file"
  ```

## Repository checkout ownership

Use this three-way decision exactly:

- If a work folder is specified, use that folder as provided. Do not clone,
  fetch, or checkout a repository.
- If no folder is specified and the task is work on a Git repository, the
  agent owns the clone. The Core Worker and backend only reserve and validate
  the workspace; they never run Git clone, fetch, or checkout.
- Otherwise, do not clone.

The session environment exposes the non-secret `CYBORGY_GITHUB_REPOSITORY` and
the compatibility hint `CYBORGY_REPOSITORY_CHECKOUT_MODE`. For new launches,
the Worker derives that hint from explicit launch provenance: a supplied work
folder means `provided`, while a temporary workspace together with a repository
fact means `agent_clone`; otherwise use `none`. The compatibility hint mirrors
that decision for older Skill consumers, but is not SessionExecution
or directory/Git authority. In `agent_clone` mode, confirm the reserved
workspace, read the short-lived `github-read-token` only through the Cyborgy
secret interface, and connect it to a temporary `GIT_ASKPASS` helper. Keep
credentials out of argv, clone URLs, prompts, logs, artifacts, and durable
plaintext fields. Remove the askpass helper and credential configuration when
the Git operation finishes. A PR review may fetch and checkout its PR head in
the agent process; a continuation reuses the existing folder and must not
reclone it. Existing files, remotes, and branches must not be deleted, moved,
or overwritten. Report clone/auth/PR-fetch failures once with a concrete next
check and bounded retry behavior.

The safe askpass sequence is: obtain the path with `cyborgy secret file
github-read-token`; create a mode-0700 temporary helper outside the repository;
return `x-access-token` for the username prompt and read the password only
from that 0600 secret file; run Git with `GIT_ASKPASS=<helper>` and
`GIT_TERMINAL_PROMPT=0`; then remove the helper and unset the Git environment.
Never put the secret in a shell command argument or use a URL such as
`https://<token>@github.com/...`.
