---
name: base-capability-boundaries
description: Use when designing, implementing, documenting, or reviewing an Agent Capability Plugin, Capability API, Core or Internal API dependency, Core Worker protocol responsibility, local external-tool integration, secret delivery, or server-versus-agent ownership boundary.
---

# Base Capability And Worker Boundaries

- An Agent Capability Plugin is optional, agent-invoked, runs locally, and returns results to the agent.
- Core Worker owns registration, heartbeat, reconnect, session commands, secret delivery, and updates.
- Capability plugins do not own server endpoints, durable run state, user-input polling, server credentials, or secret management.
- Core Worker paths and Backend Core or Internal APIs must not call, depend on, or require a Capability API. A Capability API is optional product functionality and cannot be the canonical path or recovery prerequisite for Core operation.
- When a Capability API and a Core or Internal API need the same storage, authentication, transport, or implementation logic, extract a lower-level internal interface or service and make both depend on that shared primitive. Do not make the Core or Internal API depend on the Capability API wrapper.
- Use the normal session channel for server and user communication.
