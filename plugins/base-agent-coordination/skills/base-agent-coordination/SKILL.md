---
name: base-agent-coordination
description: Coordinate existing Cyborgy Sessions through explicit Session assignments and ordinary input.
---

# Session coordination

Reuse the explicitly assigned existing Session whenever possible. Include the
exact requester Session ID in a delegation prompt when the requester expects
an outcome. The receiving agent decides whether and when to report, and sends
ordinary prompt text to that exact Session with `send_input_to_agent_session`.

Never infer a recipient from a Worker, Task, Graph, recent activity, or
directory. Never use a result envelope, WorkRun/WorkEdge identity, automatic
delivery, aggregation, or fallback routing.

The Work Graph is a non-authoritative relation view: one Worker owns one Graph,
all of its Sessions are nodes, and relations connect Sessions only.
