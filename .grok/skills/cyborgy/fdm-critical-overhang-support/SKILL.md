---
name: fdm-critical-overhang-support
description: Require and verify slicer support beneath critical FDM/FFF overhangs, arches, bridges, and downward ceilings before printing. Use whenever preparing, slicing, reviewing, uploading, or starting an FDM print whose geometry may contain 70–90 degree overhangs; do not use for resin printing or CAD work that will not be manufactured.
---

# FDM Critical Overhang Support

Treat support as a verified manufacturing requirement, not merely a slicer checkbox.

## Normalize the angle

Use this convention regardless of the slicer's UI convention:

- 0 degrees is a vertical wall.
- 90 degrees is a horizontal downward-facing ceiling.
- A critical overhang is any downward-facing local surface from 70 through 90 degrees, inclusive.

Do not copy a slicer threshold number without converting its convention. Detect the critical surfaces from the oriented model in build coordinates.

## Required workflow

1. Inspect every print object after selecting its build orientation. Identify connected critical regions, including the underside of arches and the first layers of bridges.
2. Require support under every critical region. Do not exempt a 70–90 degree region merely because the slicer labels it a bridge, removes it as a small overhang, or reports support as enabled. Only an explicit user override can waive this rule for a named region.
3. Choose a support strategy that can physically reach all regions. Use build-plate-only support only when every critical region is reachable from the plate; otherwise use support everywhere, a suitable normal/tree/organic structure, painted enforcers, a changed orientation, or a model split.
4. Slice the actual final plate with the intended printer, material, layer height, and support settings.
5. Verify the actual sliced G-code. For every critical region, confirm that support or support-interface extrusion exists below it, reaches its projected footprint, and terminates within the configured top-Z gap plus one layer of tolerance. A support setting, preview tree, or nonzero total support weight is not sufficient evidence.
6. Inspect representative layer previews at the first support layer, each critical-region contact layer, the maximum cross-section, and the first model layer above support. Confirm that arches and 90-degree ceilings are supported and that support does not create forbidden cosmetic, mating, socket, or moving-surface contact.
7. If any critical region has no verified contact, do not upload or start the print. Change the support strategy or orientation, re-slice, and repeat the verification.
8. Record the oriented model hash, G-code hash, printer/material/profile, support mode, critical-region result, estimated model/support mass, time, and preview paths. Start printing only after every required region passes.

## Fail closed

Stop before printing when the geometry-to-G-code coordinate mapping is unknown, a critical region cannot be enumerated, support contact cannot be measured, or a preview contradicts the numeric result. Never report a pass by weakening the 70-degree boundary, counting support elsewhere on the model, or assuming auto-support covered the target.

## Trigger examples

Use this skill for requests such as:

- "Slice this arch and make sure the 80–90 degree underside is supported."
- "Prepare this STL for the FDM printer."
- "The previous print failed because a horizontal ceiling had no support."

Do not trigger it for resin supports, a purely conceptual CAD review with no print preparation, or general filament-drying advice.
