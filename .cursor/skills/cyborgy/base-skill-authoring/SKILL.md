---
name: base-skill-authoring
description: Creates, updates, refactors, validates, and evaluates Agent Skills for Codex, Claude Code, and Cyborgy. Use whenever a user asks to add, make, improve, audit, or troubleshoot a Skill or SKILL.md, tune its description or trigger behavior, choose bundled scripts/references/assets, or distribute and assign the authored Skill at the correct scope. Do not use for merely installing an unchanged prebuilt Skill.
---

# Base Skill Authoring

Create the smallest reusable Skill that reliably changes agent behavior for the intended tasks and stays out of unrelated tasks.

## Establish Intent And Scope

- Extract answers from the current conversation before asking questions. Identify what the Skill enables, realistic phrases that should and should not trigger it, expected outputs, target clients and models, required tools, and intended ownership or distribution scope.
- Ask only about unresolved choices that would materially change the Skill. Proceed when the request and existing evidence already answer them.
- Treat trigger quality and task-result quality as separate requirements. A Skill can load correctly and still produce poor work, or work well when invoked explicitly but fail implicit discovery.

## Inspect Before Creating

- Search existing resolved, owned, repository, and official Skill metadata and their actual files for the same responsibility.
- Update an existing Skill when the request extends its responsibility. Create a new Skill only for an independent reusable workflow; overlapping Skills make discovery ambiguous and consume the shared metadata budget.
- Preserve shipped names, IDs, slugs, resource files, and user changes unless the requested migration explicitly replaces them.
- Check current official documentation and the Agent Skills specification when client behavior, supported metadata, or installation paths matter. Keep client-specific features separate from portable guidance.

## Design The Skill

- Default to instruction-only. Add `scripts/` for deterministic or repeatedly recreated work, `references/` for detailed knowledge loaded only when needed, and `assets/` for templates or files used in outputs.
- Give critical and fragile operations exact steps and validation. Give context-dependent work principles and decision criteria instead of over-constraining one implementation.
- Keep supporting-file links one level deep from `SKILL.md`. State what each resource contains and when to read or run it.
- Prefer one good default with a conditional escape hatch over a menu of equivalent approaches.

## Write Discovery Metadata

- Use a stable lowercase hyphenated `name` of at most 64 characters and match the directory name.
- Make `description` the primary discovery contract. Write it in third person, front-load the core action and natural user vocabulary, and include both what the Skill does and the contexts in which it should be used.
- Put all trigger and boundary information in `description`, because the body is unavailable until after selection. Keep it concise and under the Agent Skills 1,024-character limit so important terms survive client-side shortening.
- Include near-neighbor boundaries when another Skill could plausibly match. Describe the intended distinction rather than listing every unrelated task.

## Write The Instructions

- Use imperative instructions and consistent terminology. Include only non-obvious knowledge, decisions, and procedures that justify their context cost.
- Explain the reason for judgment-sensitive rules so capable models can generalize them. Reserve strict prohibitions for real safety, authorization, integrity, or format constraints.
- State inputs, outputs, decision points, failure behavior, and verification for multi-step or quality-critical work.
- Keep `SKILL.md` under 500 lines. Move conditional detail to directly linked references, and add a contents list to a long reference.
- Avoid time-sensitive facts when the Skill can require a current lookup instead.
- Start from the portable Agent Skills fields `name` and `description`. Add optional standard or client-specific metadata only when the target needs it, and never assume another client implements that extension.

## Save Through The Correct Route

- For a user-owned Cyborgy Skill, use `base-api-usage` to discover the current Skill API and schema. Preserve existing files during updates; use full file-set replacement only when deletion of omitted files is intentional.
- For an official Cyborgy catalog Skill or Skill Set assignment, follow
  `cyborgy-official-skill-maintenance`. Update the catalog source, Skill Set
  assignment, and tests through the repository PR and review flow.
- For a repository, personal client, managed installation, or plugin, use the target client's current documented location and packaging rules. Confirm the requested scope instead of silently installing it more broadly.

## Validate And Iterate

- Run a structural validator appropriate to the target and inspect the final file set for stale placeholders, broken relative links, unsupported metadata, and accidental extra files.
- Create realistic should-trigger and should-not-trigger prompts, including terse wording, synonyms, and a near-neighbor case. Test explicit invocation and implicit discovery in fresh context when the target supports both.
- Evaluate representative tasks with and without the Skill, or against the previous version, when outputs are objectively comparable. Use qualitative review for subjective outputs rather than inventing weak numeric assertions.
- Test on each intended client or model family when feasible. Client-specific loading, metadata, and invocation behavior are not interchangeable.
- Execute every bundled script on representative success and failure inputs. Make errors actionable and verify the produced artifact or state.
- Tune `description` when selection is wrong; tune the body or resources when selection succeeds but task results are wrong. Re-run the affected cases after each revision.

## Verify Delivery

- Confirm the saved or committed file set, installation or catalog
  registration, requested Skill Set assignment, and resolved Skill list as
  applicable.
- When runtime materialization matters, verify the Skill appears in a fresh compatible session; a file existing on disk or in a database does not alone prove the target agent received it.
- Report the final scope, discovery triggers, resources, validation commands, results, and any target-specific behavior that was not tested.
