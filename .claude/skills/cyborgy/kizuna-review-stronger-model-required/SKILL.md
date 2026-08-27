---
name: kizuna-review-stronger-model-required
description: Use when requesting code review, starting a code_reviewer Session, enqueueing Cyborgy PR review, or choosing a reviewer model. Forbids Fable as a reviewer and requires the strongest reachable non-Fable reviewer, falling back to a same-tier model and then to the implementer's own model rather than dropping to a weaker one.
---

# Review Stronger Model Required

## Hard rules

- Never request code review or PR review from Fable (`fable`, `claude-fable-5`, or any worker-advertised Fable alias). Fable may only be used for design when the user explicitly designates it; it is never a review executor under this Skill.
- Never route a review to a model weaker than the implementer. Dropping a tier is the one thing this Skill exists to prevent.
- Review is never optional. If the ladder below cannot be satisfied at all, report that exact blocker rather than skipping review.

## The reviewer ladder

Take the highest rung that can actually be launched with available quota. Descending a rung is
allowed only because the rung above it is unreachable, and the reason must be recorded.

1. **A strictly stronger verified model**, excluding Fable.
2. **A different verified model in the same top tier.** An Opus implementer is reviewed by Sol;
   a Sol implementer is reviewed by Opus. Prefer a different family or vendor: a same-tier
   reviewer earns its place by failing differently from the implementer.
3. **The implementer's own model, in a separate Session.** Allowed when the only models ranked
   above the implementer are Fable, and no other model of the same tier is reachable.

Rung 3 is weak review, but it is stronger than the alternative people reach for, which is
quietly handing the work to a lesser model. A same-model reviewer at least matches the
implementer's capability. Take rung 3 rather than descending a tier.

When you use rung 3, say so plainly in the review request and in the Task record: the reviewer
is the same model that wrote the change, this is a known weakness, and it is being accepted
because rungs 1 and 2 were unreachable. Give the reviewer no memory of the implementation - a
fresh Session with only the diff, the requirements and the review questions.

## Strength order for review selection

Use the selected worker's verified `model_catalogs` and exact advertised IDs. Prefer this
relative order when comparing candidates (stronger first):

1. Codex Sol (`gpt-5.6-sol`) and Claude Opus (`opus`)
2. Claude Sonnet (`sonnet`), Codex Terra/Luna / GPT-5.5-class models
3. Implementation-tier models such as Cursor Grok, Devin, Codex Spark, Haiku, and mini models

A reviewer must come from a strictly higher tier than the implementer, or be an exact
higher-ranked model in the same family when tiers would otherwise match. Sol and Opus may review
implementers in tier 2 or 3. Implementer-tier models must not review each other, except under
rung 3 above.

## Reachability is a fact, not an assumption

Before concluding that a rung is unreachable:

- A model reachable through more than one tool still counts. If the direct tool is out of quota,
  check every other tool that advertises the same model.
- An advertised model list whose `model_catalogs.status` is `fallback` is a guess, not an
  entitlement. Verify what the account can actually serve before treating an advertised model as
  reachable, and verify afterwards what actually ran.
- A quota that resets soon may make a higher rung reachable by waiting. Say what the reset time
  is and let the requester decide between waiting and descending.

## Launch checklist

1. Identify the current implementer tool/model from runtime evidence.
2. Run the normal online-worker / tool-quotas / profile preflight from `base-agent-selection` and
   `base-pr-review-requester`.
3. Filter out Fable and every candidate weaker than the implementer.
4. Take the highest reachable rung of the ladder.
5. Launch with `actor_role=code_reviewer` (or the repository's required review path).
6. Record on the Task timeline which rung you took, which model implemented, which reviewed, and
   why each higher rung was unreachable.
