---
name: simple-general-purpose-design
description: Use when designing, implementing, or reviewing architectures, algorithms, workflows, validation, or agent systems. Prioritizes the simplest solution that works across the intended use-case range, assumes cooperative agents rather than malicious behavior, makes mistakes observable and recoverable, and adds pessimistic safeguards only for concrete failures without sacrificing simplicity or general usefulness.
---

# Simple General-Purpose Design

Design the smallest coherent mechanism that satisfies the full intended use-case range. Preserve general usefulness before adding defensive machinery.

## Priority Order

Apply these priorities in order:

1. Correctly serve the stated use cases.
2. Keep the structure and execution path simple.
3. Generalize across the intended use-case envelope without special-case patchwork.
4. Make mistakes, failures, and uncertainty visible and diagnosable.
5. Add preventive safeguards only when a concrete failure justifies them.
6. Prefer pessimistic handling where it does not materially reduce simplicity, usability, or generality.

Do not reverse this order merely because a theoretically stronger guard can be designed.

## Define The Useful Range

Before adding abstractions or safeguards, state:

- the intended users and use cases;
- the inputs, environments, and scale that must work;
- assumptions that are intentionally accepted;
- failures that must be prevented;
- failures that may instead be detected, reported, and corrected.

A design is general-purpose when it works uniformly across this declared range. It does not need to solve unrelated problems or defend against threats outside the range.

Avoid both extremes:

- Do not optimize only for one demonstrated example.
- Do not expand the system to hypothetical use cases that were not requested.

## Assume Cooperative Agents

Assume agents and ordinary collaborators act in good faith unless the user, threat model, or domain explicitly says otherwise.

Do not add mechanisms whose only purpose is to prevent a cooperating agent from deliberately forging evidence, impersonating a reviewer, modifying its own approved bytes, or bypassing a process it is instructed to follow.

Agents can still misunderstand instructions, choose a wrong path, use stale files, race with another process, or make implementation mistakes. Handle these as operational errors, not adversarial attacks.

Security, authentication, authorization, untrusted input, multi-tenant isolation, compliance, destructive operations, and explicitly adversarial environments are exceptions. In those domains, use the stated threat model and required safeguards.

## Prefer Observable And Recoverable Failure

Not every possible failure must be prevented in advance. Prefer a simple path that makes ordinary mistakes evident and correctable.

For each realistic failure, choose the lightest effective response:

- prevent it when silent corruption, irreversible loss, unsafe behavior, or false success would otherwise occur;
- fail clearly when continuing would produce an invalid result;
- warn when work can safely continue but attention is needed;
- record enough context for an agent to correct the procedure and retry;
- leave irrelevant hypothetical failures outside the design.

An error or warning should identify:

- the operation that failed;
- the expected and observed state;
- the likely consequence;
- the relevant path, input, or identity;
- the next corrective action.

Never turn a recoverable mistake into a large authorization, provenance, or verification subsystem without evidence that the subsystem is needed.

## Keep Safeguards Proportional

A safeguard is justified when there is a concrete failure path within the declared use-case and threat model.

Before making it blocking, show:

- what can actually go wrong;
- how likely or repeatable it is;
- its impact;
- why existing checks would not detect it;
- why a simpler warning, validation, smoke test, checksum, isolated output, or retry instruction is insufficient.

Prefer small safeguards such as:

- validation of required inputs and types;
- compile and smoke tests of the actual execution path;
- clear nonzero exits and structured diagnostics;
- attempt-specific output directories;
- atomic publication where partial output could be mistaken for success;
- stable identities or checksums for the exact inputs and code that matter;
- idempotent or safely repeatable steps.

Do not automatically escalate to complete supply-chain proofs, exhaustive runtime attestation, cryptographic reviewer identity, nested meta-validation, or prevention of same-user malicious mutation.

## Validate The Primary Path First

Verify the simplest real execution path before proving elaborate properties about its wrappers.

Use this order unless the domain requires otherwise:

1. Parse, compile, or type-check the actual implementation.
2. Run a focused smoke test through the actual interpreter and environment.
3. Reproduce the primary success and failure paths.
4. Validate semantic correctness and important edge cases.
5. Add integration and regression coverage.
6. Add stronger provenance or hardening only for remaining concrete risks.

A review must not forbid basic execution so completely that syntax errors, wrong interpreters, missing dependencies, or unusable commands remain undiscovered.

## Prevent Review-Scope Growth

Review the requested system, not every optional mechanism a reviewer can imagine.

A finding is blocking only when it shows a concrete path, within the declared use-case and threat model, to one of the following:

- incorrect output or false success;
- loss or irreversible corruption;
- unsafe behavior;
- an unmet explicit requirement;
- a failure that existing validation cannot expose or help correct.

If a newly proposed audit or safety subsystem creates its own bugs, first ask whether the subsystem is necessary. Do not make the subsystem and all of its internal correctness conditions new acceptance requirements by default.

Distinguish:

- a bug in the primary solution;
- a bug in required evidence or recovery;
- a bug that exists only inside optional hardening.

Fix the first two. Remove or defer the optional hardening when that preserves the required behavior more simply.

## Review Checklist

Before approving a design, implementation, or review requirement, confirm:

- It covers the full intended use-case range.
- It is the simplest coherent design that meets the requirements.
- It avoids part-, model-, environment-, or example-specific rules.
- Its abstraction solves the same underlying problem wherever it is reused.
- Ordinary errors produce actionable diagnostics.
- Basic compile and smoke validation preceded elaborate formalization.
- Every blocking safeguard corresponds to a concrete in-scope failure.
- Optional hardening has not become a new source of complexity or feature loss.
- A more pessimistic design is used where it is low-cost and does not impair simplicity or general usefulness.

When these goals conflict, preserve correctness first, then simplicity and general usefulness. Add complexity only with evidence.
