---
name: base-agent-selection
description: Use when selecting a worker/tool/model, delegating, or scheduling agent work; when the current model-self is Sol (`gpt-5.6-sol`) or Fable (`claude-fable-5`) and implementation is requested, delegate implementation to another verified executor instead of implementing directly; preserves provider-unmeasurable quota=unavailable launch uncertainty, startup fallback, deadlines, and executor matching. Do not use for ordinary repository implementation without an executor-selection decision.
---

# Base Agent Selection And Delegation

Use this skill before selecting or launching another agent.

## Selection Preflight

1. Obtain raw online-worker, model-catalog, profile, and quota facts with point queries. Do not call a synthetic pool/ranking API.
2. Choose only from the returned worker/tool/model combinations. Prefer `selection_status=eligible`, match the repository and capabilities, and compare current workload. Treat `quota_unverified` as uncertainty, not as confirmed capacity or an automatic rejection.
3. Inspect `deadline_fit` and every quota metric's `remaining` and `reset_at`. Never schedule an exhausted tool whose reset is unknown or later than the task deadline. Do not infer quota numbers or reset times.
4. Immediately before launch, discover `tool-quotas` and call its `list` endpoint through `call_api` for the selected worker and tool. Reject confirmed exhausted results and any quota `error` result before launch. Treat `unavailable` as an uncertainty signal only when the provider reason explicitly states quota measurement is not available; it is not a capacity-exhaustion state. If that evidence is present and the worker, exact model, repository/checkout, capabilities, and deadline all pass, record the risk and attempt one normal launch. For unknown, stale, or absent quota with the same verified launch facts, record the uncertainty and make the same single controlled attempt.
5. Keep Role and Skill Set resolution separate from execution-resource
   selection. Preserve the execution Role and the resolved Base, Organization,
   and Repository Skill Sets; pool selection must not substitute, remove, or
   invent either one.

## Runtime Evidence Before Selection

- Call `search_apis`/`call_api(api_id="online-workers", inputs={"online_only": "true"})`. Use its current worker IDs, roots, capabilities, tools, and `model_catalogs` as the source of truth for which agent tools and exact models can run. Do not infer availability from a static preference list.
- Call `call_api(api_id="tool-quotas", endpoint_id="list", inputs={"worker_id": ..., "tool": ..., "include_offline": false})` and match each result to the same `worker_id` and tool. Inspect `status`, `fetched_at`, every quota metric's scope and remaining amount, and `reset_at`; never invent quota numbers.
- Inspect the Task `due_at` and priority, or the user's explicit deadline when there is no Task. Estimate queue delay plus task runtime before selecting an executor.
- Refresh this evidence when selection or launch has been delayed enough that online state, quota, or the deadline fit may have changed.

## Availability, Quota, And Deadline Gate

- Reject a candidate when the worker is offline, the exact tool/model is not advertised, required capabilities are missing, the repository/checkout or existing Session is not verified or conflicts, the deadline does not fit, confirmed capacity-unavailable quota is present, any quota `error` is present, or `unavailable` is present without provider-measurement-unavailable reason. These are pre-launch rejections, not fallback events.
- Prefer fresh quota status `available`. A provider-unmeasurable `unavailable` result is an uncertainty signal, not proof of exhaustion: record the reason, keep the exact worker/tool/model identity, and make exactly one controlled launch attempt after all other gates pass. An `unknown`, `stale`, or absent quota result follows the same one-attempt rule when the exact launch facts are verified; do not claim either case is `available` or `eligible` merely because it can be tried.
- After an uncertain-quota launch, use `wait_agent_session_start`. If startup or provider admission returns a concrete error or stop before `native_started`, record the exact failure and move once to the next ordered candidate. Do not retry the same candidate without changed evidence.
- Once `native_started` or implementation work has begun, do not blindly launch a duplicate fallback. Inspect the Session and checkout state first, then resume, stop, or reassign deliberately.
- `native_started` is the only startup-success state. A later provider/work error after `native_started` does not authorize a duplicate fallback launch.
- Interpret quota metrics by scope. A model-specific exhausted window blocks only that model; an exhausted shared tool window blocks the tool. If scope is unclear, do not claim the candidate is available.
- Compare quota reset time, queue delay, and expected runtime with `due_at` or the explicit deadline. Do not launch a candidate that cannot reasonably finish in time when another verified candidate can.
- If no candidate passes the gate, report the concrete availability, quota, capability, or deadline risk. Unknown quota alone is not a blocker to the single controlled launch attempt described above.

## Controlled unavailable-quota fallback contract

The following states must remain distinct:

- `unknown`, `stale`, or absent quota: measurement is incomplete; with verified launch facts, record uncertainty and try once.
- provider-unmeasurable `unavailable`: the provider cannot measure quota; with verified launch facts, record that risk and try once.
- confirmed `exhausted`: capacity is known to be unavailable; reject before launch, respecting metric scope and reset/deadline fit.
- quota `error`: the quota fact is unusable; reject before launch.
- concrete startup/provider error before `native_started`: record the failure and advance exactly once to the next ordered candidate.
- `native_started`: success; never create a duplicate fallback for the same work.

This Skill should trigger for worker/tool/model selection, delegation, or scheduling. It should not trigger for ordinary implementation, testing, or documentation work that has no executor-selection decision.

## Task Phase And Model Selection

- Classify the Task by design readiness before considering implementation difficulty. Inspect its summary, attachments, planning criteria, recorded approvals, and unresolved specification or architecture decisions. A `todo` label alone does not mean implementation-ready; Task status grants no execution authority.
- When design readiness is uncertain, treat the Task as still needing design. Do not send an underspecified Task directly to an implementation-only executor merely because its requested code change looks small.
- When requirements, architecture, user-visible behavior, acceptance criteria, task breakdown, or another material design decision is still unresolved, prefer Codex with `gpt-5.6-sol` as the default design candidate. Use only an exact combination from the selected worker's verified model catalog. Sol is a design executor: it may plan, design, break down work, and review, but must not implement.
- Select Claude Fable (`claude-fable-5` in role settings; use the exact candidate ID advertised by the worker, normally `fable`) for design work only when the user has explicitly designated Fable for the current work or Session — a specific instruction naming Fable in the current user message, the Task summary, or a recorded Session rule. An idle worker, exhausted or unverified Sol quota, an unresolved-design default, a review or QA step, a delegation target, or a mixed Task's design portion is never by itself an explicit designation, and none of those situations may select or launch Fable without one. Absent an explicit designation, use Sol (or another verified design candidate) instead of Fable for unresolved design, review, QA, fallback, or quota substitution.
- When the user has explicitly designated Fable, it remains a design executor under the same boundary as Sol: it may plan, design, break down work, and review, but must not implement. The normal online-worker, exact tool/model/profile, quota, and capability/deadline preflight elsewhere in this Skill is still required before launch; explicit designation selects the candidate, it does not skip preflight.
- If a user-designated Fable fails preflight or a launch attempt (offline worker, unadvertised exact model, missing capability, confirmed exhausted quota, deadline miss, or a concrete startup failure), do not silently substitute another model. Report the exact rejection or failure reason to the user instead of falling back on their behalf.
- When design and acceptance criteria are settled, classify the implementation by functional scope before choosing an executor:
  - A **single-feature change** stays inside one product capability or one vertical feature boundary. A compact change of roughly five files or fewer is supporting evidence, but file count alone does not determine the class. Use Devin with `swe-1-7` first, then Codex with the exact worker-advertised GPT-5.3 Codex Spark model (normally `gpt-5.3-codex-spark`).
  - A **multi-feature change** coordinates two or more product capabilities, feature boundaries, public contracts, or independently owned components. Use Codex with `gpt-5.6-luna` and `thinking_depth=medium` first, then Claude Sonnet (`claude-sonnet-5` in role settings; use the exact advertised candidate, normally `sonnet`).
- Select the first exact combination in the applicable scope order that passes preflight. Do not guess an identifier when the worker advertises a different one.
- If a preferred implementation candidate is rejected for offline worker, exact tool/model is not advertised, required capability, confirmed capacity-unavailable quota, quota `error`, or deadline miss; or returns an assignment or concrete startup/provider error before `native_started`, record the concrete rejection or failure reason in the Task timeline, refresh online-worker and tool-quota evidence, and continue to the next executor in the applicable scope order. For provider-measurement-unavailable quota status=unavailable, unknown, stale, or absent quota, attempt one launch before falling back as specified above. Do not repeatedly retry the same failed candidate without changed evidence, and do not leave the Task abandoned while a later candidate can safely meet it.
- For a mixed Task sent first to Sol, or to Fable only when the user explicitly designated Fable for that Task or Session, preserve the original implementation requirement. The design executor must record the resulting specification and then delegate or route implementation to a verified implementation model under the section below; design completion alone must not be reported as completion of the implementation Task.
- If preferred quotas are exhausted, the exact preferred model is not advertised, or the exact preferred model has confirmed capacity-unavailable quota, use a verified alternative that preserves the same boundary: Sol, or user-designated Fable per above, for unresolved design work, or an implementation-capable model for implementation-ready work. Do not substitute Fable for an exhausted or capacity-unavailable Sol without a separate explicit user designation. Record the concrete fallback and deadline risk. Never guess tool or model identifiers.

## Sol Or Fable Self-Model Implementation Delegation

- If the current session is running as Codex Sol (`gpt-5.6-sol`) or Claude Fable (`claude-fable-5`), you personally may only do abstract work: planning, design, task breakdown, and review. Never write the implementation yourself.
- Delegate all implementation work to another agent using the functional-scope classification and ordered candidates above: Devin then GPT-5.3 Codex Spark for a single-feature change; `gpt-5.6-luna` with `thinking_depth=medium` then Claude Sonnet for a multi-feature change.
- Set the delegated implementation agent's reasoning/thinking effort to High or above whenever the verified model catalog exposes that control.
- Follow the Delegation section below for Task creation and launch mechanics; verify the exact tool/model and quota before launch like any other delegation.

## Delegation

- Launch delegated Codex, Claude, Cursor, Grok, or other agents only through
  the Cyborgy Session primitives described below. Never start a delegated
  executor as a local nested subprocess with `cyborgy codex exec`, raw
  `codex exec`, `cyborgy cursor-agent`, or an equivalent shell command.
  Local nesting bypasses the Worker's Session policy propagation and can
  combine `workspace-write` with `approval=never`, leaving the child unable
  to create its sandbox or request the permission it needs.
- Do not diagnose that policy mismatch as a repository blocker and do not
  retry it by requesting explicit escalation. When approval policy is
  `never`, an escalation request is invalid by construction. Use
  `start_agent_session` so the Worker applies the selected Session's
  sandbox and approval policy consistently; if launch still fails, report
  the exact Session launch error and select another verified candidate.
- When a repository is cloned before starting a Session, always set the Session working directory (`dir`) to the exact cloned repository directory. Repository Agent Settings and their Skills are selected from the Git remote detected there; starting the Session outside the cloned directory prevents those repository-linked Skills from being provided.
- Create the Cyborgy Task first and include its ID, objective, verified context, constraints, acceptance criteria, and expected deliverable in the request.
- Verify the worker, exact tool/model/profile, capabilities, quota scope, and deadline fit immediately before launch using raw facts and the `tool-quotas` Catalog endpoint.
- Match repository, checkout, workload, capabilities, and task type. Do not choose an arbitrary idle agent.
- Include the Task `due_at` or explicit deadline, selected tool/model, quota evidence, and fallback in the delegated prompt.
- For immediate new-session delegation, use the direct `start_agent_session` path; use the exact assignment primitive only for its corresponding Task route, and treat an existing-session follow-up as a separate explicit path.

## Scheduled Delegation

- Use the `agent-scheduler` Catalog `create` endpoint only when `run_at` must be in the future or the user needs recurrence. Use `start_agent_session` for immediate work.
- Before creating or re-enabling a schedule, refresh `call_api(api_id="online-workers", inputs={"online_only": "true"})` and `call_api(api_id="tool-quotas", endpoint_id="list", inputs={..., "include_offline": false})` for the exact worker, tool, model, profile, and deadline just as for an immediate launch.
- Set `interval_seconds` above zero only when the user explicitly requested recurring execution or already granted permission for it.
- Discover `agent-scheduler` with `search_apis` and use its `list`, `get`, `create`, `update`, `enable`, `cancel`, and `delete` endpoints through `call_api`. Do not assume schedule operations are direct MCP tools.
- Preserve the exact returned schedule ID. Use the dedicated enable/disable and cancel transitions instead of writing a status field through a generic update.
