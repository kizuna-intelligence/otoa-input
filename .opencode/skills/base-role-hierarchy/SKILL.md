---
name: base-role-hierarchy
description: Use when interpreting, inspecting, assigning, or debugging Cyborgy Roles, the Base, Organization, and Repository Skill Sets, Agent Settings, resolved skills, or legacy role_ids and parent_role_ids compatibility fields.
---

# Roles, Skill Sets, And Agent Settings

Use this Skill before interpreting or changing Roles, Skill Sets, or Agent
Settings.

- A Role represents the position or responsibility an agent has during one
  execution, such as Task Agent, Task Reviewer, Code Reviewer, or Worker
  Manager. It is not a collection of Skills.
- Skill Sets are reusable collections of Skills. The live configuration has
  exactly one system-wide Base Skill Set, one Organization Skill Set per
  organization, and one Repository Skill Set per repository.
- Do not create separate Skill Sets for Task Agent, Task Reviewer, Code
  Reviewer, Worker Manager, or another execution Role.
- The server flattens Skills in Base -> Organization -> Repository order and
  removes duplicates by keeping the first occurrence. The Core Worker uses the
  resolved list without reinterpreting the three layers.
- Users do not create arbitrary Skill Sets. Change only the Organization or
  Repository Skill Set named by the user; a repository-layer request does not
  authorize changing the Organization or Base Skill Set.
- Role and Skill Set are fixed together when an instruction is accepted.
  Later Skill Set changes apply only to newly accepted instructions.
- `role_ids`, `parent_role_ids`, Role-shaped records, and the technical Skill
  ID `base-role-hierarchy` are legacy compatibility details. Do not present
  them as the current Skill Set model or use them to create purpose-specific
  Skill Sets.
- Folders do not own Agent Settings.
- Inspect the exact Skill Set scope and ID, its direct `skill_ids`, and the
  resolved Skill origins, versions, and content hashes before changing it.
