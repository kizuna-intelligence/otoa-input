---
name: base-tunnel-exposure
description: "Official Cyborgy development skill for using paid short-lived HTTPS tunnels safely through the Temporary URL Capability API."
---

# Base Tunnel Exposure

Use this skill when you need to expose a local development server or callback endpoint through a temporary public HTTPS URL.

## What Cyborgy Provides

Cyborgy tunnels are self-hosted ephemeral reverse tunnels, not Cloudflare Tunnel:

- The `temporary-url` Capability API is available only on paid plans.
- The generic MCP `call_api` runner starts `frpc` on the worker machine without returning the shared tunnel credential to the agent.
- Traffic flows through the Cyborgy frps VM and Caddy to a compact `https://<sub>.t.cyborgy.dev` URL when the backend environment is configured.
- The local tunnel and its credential file are removed automatically when the requested TTL ends.

## Capability API

Discover the schema with `search_apis`, then call:

```text
call_api(
  api_id="temporary-url",
  endpoint_id="create",
  inputs={"local_port": 3000, "ttl_minutes": 5}
)
```

`local_port` is required. `ttl_minutes` defaults to 5 and must be between 1 and 60. The result contains the public URL, local port, expiry, and active process facts. Do not use a Cyborgy CLI command or a dedicated direct MCP tool for temporary URLs.

## Safety Rules

- Only expose the specific local port needed for the task.
- Do not expose admin consoles, credential stores, cloud metadata services, databases, or unauthenticated destructive tools.
- Assume the generated URL is publicly reachable while the tunnel is open.
- Choose the shortest practical TTL; the tunnel closes automatically.
- If the API reports that a paid plan is required, report that entitlement fact and do not substitute another public tunnel provider.
- If the backend reports `tunnels are not configured on this backend`, do not improvise with unrelated public tunnel services unless the user explicitly asks. Report that the Cyborgy tunnel backend is not configured for the current environment.
