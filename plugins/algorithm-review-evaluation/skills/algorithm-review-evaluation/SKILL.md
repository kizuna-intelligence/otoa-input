---
name: algorithm-review-evaluation
description: Use when reviewing algorithmic, performance-sensitive, or decision-making code changes. Enforces evidence-based review, rejects unjustified parameter changes, rule-based fallback logic and patchwork fixes, and requires complete requirement coverage.
---

# Algorithm Review Evaluation

Use this skill during code review for algorithmic logic, ranking/scoring, matching, inference, parsing, scheduling, optimization, fallback behavior, parameter changes, or any change where correctness or performance can be degraded by ad hoc logic.

## Review Stance

- Lead with concrete findings ordered by severity.
- Cite file and line references for every actionable finding.
- Treat vague justification as insufficient when the code changes behavior, correctness, performance, or maintainability.
- Ask for changes when a finding affects correctness, performance, security, reliability, or requirement coverage.

## Required Checks

1. Requirement coverage
   - Verify that all stated requirements are implemented.
   - Flag missing requirements, silently narrowed scope, or behavior that only works for the demonstrated happy path.

2. Evidence-based changes
   - Check that each substantive change has a clear reason grounded in tests, data, API contracts, user-visible behavior, or system constraints.
   - Flag changes that appear arbitrary, cargo-culted, or detached from the actual failure mode.

3. Parameter changes
   - Treat parameter changes as behavior changes. This includes thresholds, timeouts, model names or versions, decoding settings, prompt limits, batch/concurrency limits, routing weights, retry counts, silence durations, VAD/VAP settings, memory limits, GPU utilization, and feature flags.
   - Require the author to identify the previous value, the new value, why the old value is insufficient, and what evidence supports the new value.
   - Evidence may be tests, logs, metrics, production data, documented API limits, or a narrow deterministic contract.
   - If the evidence is missing, require an explanation. If the explanation is not technically reasonable, do not allow the parameter change. Request changes until the rationale is reasonable or the parameter change is reverted.
   - Require validation that the new value does not degrade existing behavior or performance.

4. Algorithm integrity
   - Reject brittle rule-based logic, magic thresholds, hard-coded special cases, and low-quality fallback paths when they replace a more principled implementation or degrade expected performance.
   - If a rule-based or fallback path is introduced, require a concrete explanation of why it is necessary, what alternatives were considered, and how it is validated.
   - If the explanation is absent or not technically convincing, request changes.

5. Patchwork fixes
   - Treat patchwork fixes as acceptable only when there is a legitimate reason and no realistic cleaner approach for the current change.
   - A legitimate reason must be visible in code comments, tests, commit/PR context, or the reviewer discussion.
   - If a patchwork fix lacks an explanation, flag it. If the author cannot provide a technically sound reason, request changes.

6. Tests and validation
   - Require tests or reproducible validation for algorithmic behavior, edge cases, parameter changes, and fallback paths.
   - Check whether appropriate integration tests were run or added when the issue spans modules, APIs, persistence, model/inference boundaries, streaming behavior, deployment wiring, or user-visible workflows.
   - Verify that test coverage maps to the actual issue or user request, including the failure mode, important edge cases, and regression risk.
   - If integration coverage is absent or too narrow for the issue, request additional integration tests or equivalent reproducible validation.
   - Flag tests that only snapshot the new implementation without proving the intended behavior.

## Findings Template

When raising an issue, state:

- What changed and why it is risky.
- Which requirement, invariant, or performance expectation is affected.
- What evidence is missing or contradictory.
- What kind of fix would be acceptable without prescribing a brittle implementation unless the correct approach is clear.

## Approval Bar

Do not approve algorithmic changes when:

- Requirements are only partially satisfied.
- Parameter changes lack a specific, technically reasonable, evidence-backed rationale.
- Rule-based or fallback logic degrades quality without a strong, specific rationale.
- The patch is a workaround with no comment, test, or documented constraint.
- Integration test coverage is missing or insufficient for the issue and no equivalent reproducible validation is provided.
- The author cannot explain why the chosen approach is the correct or least-bad realistic option.
