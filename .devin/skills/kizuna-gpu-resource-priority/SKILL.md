---
name: kizuna-gpu-resource-priority
description: Use when planning or executing GPU-accelerated training, inference, benchmarking, profiling, rendering, diffusion, simulation, or other compute workloads for Kizuna Intelligence, or when distinguishing Kizuna shared GPU hosts from Algomatic Dynamics 1号機. Prioritizes available organization GPUs, requires capacity and conflict checks, and prevents host-ownership misrouting.
---

# Kizuna GPU Resource Priority

Use the organization's shared GPU resources as the first choice for workloads that can materially benefit from GPU acceleration.

## Host identity and ownership

- **Algomatic Dynamics 1号機** / **Algomatic 1号機** is `ad-rtx4090-02` (Tailscale IPv4 observed as `100.82.177.123`, login user `multisense-ai`) with 1 NVIDIA GeForce RTX 4090 24 GiB.
- Algomatic 1号機 is **not** `192.168.1.17` and is **not** `192.168.1.15`.
- `192.168.1.17` and `192.168.1.15` are Kizuna shared GPU hosts. Do not relabel either one as an Algomatic machine.
- The Dino GEAR-SONIC full-body point-navigation workload (`train_walking_sonic_fullbody.py`) is associated with Algomatic 1号機. Distinguish it from the ordinary velocity-command walking task.

Treat hostnames, addresses, hardware, and availability as routing guidance that must be verified immediately before use.

## Available Kizuna GPU hosts

- `192.168.1.17`: 2 NVIDIA GPUs. Inspect the current models, VRAM, jobs, and availability before selecting a device.
- `192.168.1.15`: 1 NVIDIA GPU with 16 GiB VRAM and 2 AMD GPUs with 32 GiB VRAM each. Inspect the current models, jobs, and availability before use.

## Required workflow

1. Resolve the requested organization and machine label before choosing a host. For "Algomatic 1号機", select and verify `ad-rtx4090-02`; never substitute a Kizuna shared host merely because it has a suitable GPU.
2. For Kizuna training, inference, benchmarking, profiling, rendering, diffusion, simulation, or other GPU-suitable compute, consider the Kizuna shared hosts before local CPU execution, cloud compute, external GPUs, or other non-GPU paths.
3. Inspect workload compatibility, current utilization, free VRAM, existing jobs, and device identity before choosing a host or GPU. Pin the workload to an exact device when practical.
4. Select a compatible and available organization GPU first. For AMD ML workloads involving ROCm, PyTorch, Transformers, vLLM, AOTriton, AITER, attention, GEMV, or diffusion, also follow the `amd-gpu-rocm-stack` skill.
5. Do not silently fall back. If a task could materially benefit from these GPU resources but you intend not to use them, pause before execution and consult the user. Explain why, the tradeoff, and the available alternatives.
6. Consultation is not required when the work is genuinely CPU-only or so small that GPU use provides no material benefit. If this is ambiguous, consult the user.
7. For Isaac Sim and Isaac Lab, two or more consumers or observed instances are not by themselves a prohibition. First discover and use a common-use path such as a persistent Isaac Kit service, motion MCP, pose stream, or shared renderer, and attach clients/tasks to that service instead of starting another independent Kit process. Count independent Kit processes separately from clients attached to one shared service.
8. Do not interrupt another job without explicit user authorization for that exact instance. When authorization is given, re-resolve the command, owner, and PID immediately before stopping it, terminate only the exact PID, and never use broad or pattern-based process termination.
9. Report the selected organization, host, GPU vendor, exact device when known, relevant VRAM/availability evidence, the common-use path, and the selection rationale. Do not invent unspecified hardware details.
