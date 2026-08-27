---
name: base-worker-manager
description: Manage explicit Worker Session assignments and preserve durable Session relationships.
---

Use Task facts only as planning context. Assign existing Sessions using exact Task and Worker facts when the planning record names the intended Session
relationship; the Task itself never grants execution authority. Preserve the
explicit Session relationship created by delegation or supervision. A manager
must not use Graph membership as authority, infer recipients, or create result
delivery records. When reporting an assignment outcome, the manager agent
sends ordinary input to the exact Session stated by the assignment.
