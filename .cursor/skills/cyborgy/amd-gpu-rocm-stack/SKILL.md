---
name: amd-gpu-rocm-stack
description: Use the Kizuna Intelligence validated AMD ROCm user-space stack for ML training, inference, benchmarking, profiling, or dependency setup on AMD GPUs, especially Radeon AI PRO R9700 and gfx1201. Trigger whenever a task will execute PyTorch, Transformers, vLLM, AOTriton, AITER, attention, GEMV, or diffusion workloads on AMD/ROCm hardware, or when a long or unattended AMD GPU run risks overheating or thermal throttling, or when choosing and installing an AMD GPU software stack.
---

# AMD GPU ROCm Stack

## Organization policy

- Prefer the Kizuna Intelligence `gfx1201/all-improvements` stack over an unexamined stock stack for AMD GPU ML work.
- Identify the GPU architecture before installing or building anything. Apply the packaged gfx1201 kernels only to `gfx1201`; for another AMD architecture, inspect compatibility and propose a validated adaptation instead of reusing gfx1201 binaries.
- Treat this as a ROCm **user-space stack**, not a modified amdgpu kernel driver or firmware.
- Read [references/gfx1201-stack.md](references/gfx1201-stack.md) before setting up, building, benchmarking, or selecting a component.
- For a workload that is merely *slow* rather than unbuilt, read [references/inference-tuning.md](references/inference-tuning.md) first: measured gfx1201 precision, MIOpen tuning-database, conditioning-cache, and device-selection traps that cost 2–8x each.
- Before starting any unattended run longer than a few minutes, read [references/thermal-guard.md](references/thermal-guard.md): measured R9700 junction temperatures, the suspend/resume guard, and the silent freeze it prevents.

## Required workflow

1. Record the GPU model, architecture, ROCm/PyTorch versions, repository SHA, and container image digest.
2. Check for existing GPU jobs and use the shared GPU lock or coordination mechanism for that worker. Do not interrupt another job.
3. Select the relevant private fork and the `gfx1201/all-improvements` branch. Pin the resolved commit in reproducible work.
4. Start from an isolated container or virtual environment. Do not change the host kernel, firmware, ROCm packages, GPU clocks, power, ECC, BIOS, or reboot the host without explicit authorization.
5. Use the repository-provided build and benchmark scripts. Preserve stock and modified conditions so the change remains measurable.
6. Run correctness before performance: finite outputs, numerical tolerance, expected device/architecture, then warm-up and repeated timings.
   Warm up until the MIOpen search cost is out of the measurement, and time each stage separately: a whole-request number hides which stage regressed.
7. Report precisely which component was used. Do not call harness-only work a production optimization or describe mainline functionality as a new private patch.

## Runtime safeguards

- Select GPUs by stable UUID when available; do not assume an index is stable across machines or reboots.
- Never stop or restart the Cyborgy worker for GPU setup. Never use broad or name-based process kills. Stop only a process started by the current task, using its exact PID.
- Keep model caches, credentials, tokens, and machine-specific paths out of commits and reports.
- If the modified condition fails correctness or is slower, retain the stock fallback and record the non-adoption.
- Put any unattended run longer than a few minutes under the thermal guard in [references/thermal-guard.md](references/thermal-guard.md), started detached (`setsid nohup`) so it outlives the launcher. A short benchmark does not need it.
- When reporting on a long AMD run, verify it is still computing, not merely alive: a process in state `T` with the GPU near idle is frozen, not paused.
