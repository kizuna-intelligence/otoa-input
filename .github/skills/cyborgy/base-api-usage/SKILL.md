---
name: base-api-usage
description: "Official Cyborgy base skill for finding deployed APIs, including capabilities named in Skills but not exposed as direct MCP tools, reading input schemas, calling APIs through MCP, and respecting repository API scope."
---

# Base API Usage

Use this skill when a task can be done through Cyborgy extension APIs, Mini App APIs, or project-scoped APIs.

## Discovery

- When the user asks for something you cannot do directly with the currently available tools, do not stop at "I cannot". First search for a Cyborgy skill, project API, or extension API that can do it.
- Check the currently available Skill list and the Skills materialized from the
  resolved Skill Set for a relevant workflow. If the needed Skill may have
  changed, use `sync_workspace` to materialize the latest Skill Set settings
  before deciding it is unavailable.
- Do not assume that a tool or capability named in a Skill is exposed as a
  first-class MCP tool. The name may identify an extension API and therefore
  may be absent from the direct MCP tool list.
- When a named tool or capability is not available directly, search for its
  exact name and responsibility with `search_apis` before declaring it
  unavailable. If it is an extension API, inspect the returned schema and
  invoke it through `call_api`.
- Use `search_apis` to find APIs by name, description, category, or tag.
- Do not expect Internal Cyborgy Session/Worker runtime operations in the
  extension API catalog. Agent quota and Workers-grid display state are
  Catalog APIs; Session user rules remain first-class MCP tools. Follow
  `base-mcp-basic-usage` for the exact boundary.
- Discover `project-settings`, then use
  `call_api(api_id="project-settings", endpoint_id="get")` to inspect
  `.cyborgy/settings.json` for APIs allowed in the current repository.
- Use `sync_workspace` when Skill Set assignments changed and the current
  folder should materialize the latest Skills.
- If a matching skill exists but is not currently installed or synced, report that exact gap and use the Cyborgy MCP workspace/project tools when available to sync it instead of inventing an ad hoc workflow.

## Invocation

- Prefer `call_api(api_id, inputs)` over ad hoc HTTP when a matching Cyborgy API exists.
- Read the API input schema returned by `search_apis` or the API detail before calling it.
- When the user has not specified a GitHub tool, prefer the official
  `github-rest-api`: discover it with `search_apis`, then invoke it through
  `call_api`.
- If the selected GitHub tool is unavailable or fails because its connection
  or authentication cannot be used, do not stop there. Search for the
  official `github-rest-api` with `search_apis`, then invoke it through
  `call_api`.
- For file inputs, pass a local file path; the MCP runner reads and uploads the file.
- Respect pricing and billing errors. If an API fails because of credits, permissions, or connection setup, report that exact blocker instead of retrying blindly.

## Scope And Safety

- Project API allow-lists live in `.cyborgy/settings.json` and are managed
  separately from Roles and Skill Sets. Do not manually widen allowed APIs
  unless the task requires it.
- Do not send secrets, private keys, raw credentials, or unrelated local files to an API.
- For GitHub writes, follow the Cyborgy GitHub App-backed API rules from the reviewer skill when that skill is present; do not write with a personal account.
- Prefer official APIs for Cyborgy-owned workflows when available.
