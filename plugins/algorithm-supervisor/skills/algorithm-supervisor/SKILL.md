---
name: algorithm-supervisor
description: Use when supervising algorithmic, model, performance, inference, or decision-logic changes to ensure the worker inspected evidence, avoided guesswork, validated behavior, preserved existing performance, and did not add unacceptable fallbacks or brittle rule-based logic.
---

# Algorithm Supervisor

Use this skill when reviewing work that changes algorithms, model behavior, thresholds, inference paths, routing, performance-sensitive code, or decision logic.

## Core Duty

Do not accept guesswork. The worker must have understood the existing behavior, made a justified change, and validated that it works without degrading existing behavior.

## Required Review Checks

1. Inspect the work history and diff. Do not rely only on the worker's final summary.
2. Confirm the worker read the relevant code and configuration before changing it.
3. Confirm the worker formed a concrete hypothesis from evidence, not from assumption.
4. Confirm the worker validated the change with appropriate tests, integration checks, logs, metrics, or live probes.
5. Confirm integration tests were run or added when the issue crosses modules, APIs, persistence, model/inference boundaries, streaming paths, deployment wiring, or user-visible workflows. If integration coverage is missing or too narrow for the stated issue, fail the review and require the worker to add or run appropriate integration tests.
6. Confirm test coverage maps to the actual issue or user request, including the failure mode, important edge cases, and regression risk. If the coverage does not sufficiently cover the issue, require additional tests or reproducible validation before passing.
7. Confirm the worker preserved existing behavior and performance unless the user explicitly approved a tradeoff.
8. Check for hidden changes to model selection, thresholds, prompt shape, streaming semantics, concurrency, fallback behavior, or endpoint routing.
9. Check that any benchmark or latency claim includes concrete measurement method and numbers.
10. For any parameter change, confirm the worker provides a specific rationale grounded in evidence. If the rationale is missing or not technically reasonable, do not allow the parameter change; fail the review and require explanation, redesign, or reversion until the rationale is reasonable.

## Parameter Changes

Treat parameter changes as behavior changes. This includes thresholds, timeouts, model names or versions, decoding settings, prompt limits, batch/concurrency limits, routing weights, retry counts, silence durations, VAD/VAP settings, memory limits, GPU utilization, and feature flags.

A parameter change is acceptable only when the worker shows all of the following:

- The prior value and new value.
- The reason the prior value is insufficient or wrong.
- Evidence supporting the new value, such as test results, logs, metrics, production data, documented API limits, or a narrow deterministic contract.
- Validation that the new value does not degrade existing behavior or performance.
- A rollback path or clear way to restore the prior value when the risk is operational.

If evidence is absent, force the worker to explain the change. If the explanation is not reasonable, the parameter change must not be approved. Require the worker to revert it or gather enough evidence and rerun validation.

## Regressions And Performance

Fail the review if the change may degrade existing functionality and the worker did not prove otherwise. Examples:

- Increased latency, longer waits, less streaming, slower first-token or first-audio response.
- Lower model quality, changed model/version/checkpoint, changed decoding parameters, or changed thresholds without evidence.
- Reduced concurrency, new serialization, new global locks, or longer resource retention.
- Changed routing, ports, env vars, or deploy targets without a rollback path.
- Weaker tests that only make failures harder to observe.

## Fallbacks

Fallbacks are not allowed unless the user explicitly asks for one and the fallback is clearly isolated, observable, and tested. A fallback must not silently hide a broken primary path.

Fail the review if the worker adds a fallback that masks errors, changes output semantics, or lets broken behavior appear successful.

## Rule-Based Logic

Rule-based logic is only acceptable when it is expected to cover at least 99% of the intended cases and this claim is supported by evidence, tests, production data, or a narrow deterministic contract.

Fail the review if rule-based logic is used as a cheap patch for model, algorithm, parsing, timing, or state bugs without strong coverage evidence.

## Validation Standard

The worker must show enough evidence that another engineer can reproduce the conclusion. Prefer:

- Focused unit tests for the touched logic.
- Integration tests for API, streaming, deployment, and model paths.
- Integration tests or equivalent end-to-end validation that cover the actual issue, not only the new implementation's happy path.
- Before/after measurements for performance-sensitive changes.
- Live health checks and logs for deployed services.
- Explicit negative tests for fallback and error paths.

## Verdict Guidance

Pass only when the change is justified, verified, and non-regressive.

When failing, tell the worker exactly what evidence is missing or what must be reverted, retested, or redesigned. If integration test coverage is absent or insufficient for the issue, explicitly require the worker to add or run the needed integration tests and verify the result. Require rerun and verification before the worker reports completion.
