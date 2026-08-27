---
name: base-browser-video-ui-testing
description: "Official developer skill for deterministic browser motion checks, Playwright video artifacts, changed-path cost gating, and structured Vertex AI UI evaluation."
---

# Browser Video UI Testing

Use this skill to turn browser motion and interaction recordings into enforceable Playwright regression checks.

## Test Structure

- Keep deterministic assertions primary: verify visible states, geometry, intermediate animation samples, final positions, and expected actions.
- Enable Playwright video for the targeted scenario, close the page before saving, and keep the video plus the evaluation JSON as test artifacts.
- Send the saved video and an explicit chronological rubric to a multimodal model. Require structured JSON with an overall pass or fail verdict, evidence for every check, observed actions, and confidence.
- Fail the Playwright test when deterministic checks fail, Vertex is unavailable, the response is malformed, the model verdict fails, or any required check fails.
- Do not let the model replace assertions that the browser can measure directly.

## Cost Gate

- Do not run paid video evaluation on every test invocation.
- Detect changed files against the pull request base plus staged and unstaged changes.
- Run the expensive check only when files affecting the target UI, layout, touch handling, animation, or shared styling changed.
- Provide explicit force and disable environment overrides for reproduction and emergency cost control.
- When git metadata is unavailable, skip paid evaluation unless the caller explicitly forces it.
- Avoid automatic retries around the whole video test; a retry would repeat the paid model call. Limit only transient HTTP retries inside the API client.

## Vertex AI Authentication

- In a developer environment, obtain a short-lived token with gcloud auth print-access-token. Never save access tokens, service-account keys, or raw credentials in the repository or artifacts.
- Allow a short-lived VERTEX_ACCESS_TOKEN environment value for isolated containers, together with VERTEX_PROJECT_ID.
- Use the Vertex generateContent REST endpoint so the same evaluator works without Application Default Credentials or a language SDK.
- Default to gemini-3.1-flash-lite. The Cyborgy dev project has verified this model; gemini-3.1-flash currently returns model-not-found. Allow VERTEX_VIDEO_MODEL to override the default when availability changes.
- Use global as the default location and allow VERTEX_LOCATION to override it.

## Evaluation Prompt

- State the exact expected action order and visible success conditions.
- List concrete failure modes such as teleporting geometry, blank frames, duplicate pages, clipping, overlap, missing controls, and incomplete actions.
- Ask for evidence grounded only in the video.
- Include the prompt, model, project, location, video SHA-256, timestamp, and parsed response in the saved JSON artifact.

## Safety

- Prefer deterministic fixtures over live user data for repeatable video tests.
- If a test needs an authentication injection point, restrict it to exact loopback hostnames and require an explicit injected token. Add a unit test proving deployed hosts cannot activate the bypass.
- Do not upload recordings containing secrets, personal data, production sessions, or unrelated screen content.
