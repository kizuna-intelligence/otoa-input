# PyTorch inference tuning on gfx1201 (R9700)

Measured on `kizuna-ai-server-2` (Radeon AI PRO R9700, gfx1201, 31 GB VRAM, ROCm 7.2.0,
PyTorch 2.11.0+rocm7.2, HIP 7.2.26015) with IrodoriTTS v4.1-Small: a 12-layer d=1280 DiT
(40 Euler steps, independent CFG) plus a conv-heavy DACVAE decoder, 4.2 s of 48 kHz audio.

An R9700 is roughly RTX 3090 class for this workload. If it measures an order of magnitude
slower, the cause is almost always the configuration below, not the hardware.

## Rule 1: never run fp32 by default

RDNA4 fp32 throughput is a trap. The same graph in bf16, one variable changed:

| Stage | fp32 | bf16 | Ratio |
| --- | ---: | ---: | ---: |
| DiT sampling, 40 steps | 8,682 ms | **1,164 ms** | 7.5x |
| Duration predictor | 622 ms | **81 ms** | 7.7x |
| DACVAE decode (4.2 s audio) | 3,598 ms | **1,581 ms** | 2.3x |
| Reference encode | 3,486 ms | 1,667 ms | 2.1x |

Set both `model_precision` and, where the codec tolerates it, `codec_precision` to bf16, and
verify task quality (here: CER over 20 sentences, same seeds) against the fp32 control before
adopting. Report the quality control alongside the speedup.

## Rule 2: give MIOpen a persistent tuning database

The first convolution call after a fresh process/cache pays a full MIOpen search:

- first decode call: **100–147 s**
- subsequent calls: 1.3–2.5 s

Without a persistent database this cost is paid again on every process start, which looks like
"the GPU is hopelessly slow" in any short benchmark or RL rollout loop. Always export:

```bash
export MIOPEN_USER_DB_PATH="$HOME/<workdir>/miopen"
export MIOPEN_CUSTOM_CACHE_DIR="$HOME/<workdir>/miopen"
```

and warm the kernels once before timing. `torch.backends.cudnn.benchmark = True` gives a further
~20% on the decode path.

Watch for this warning: it means MIOpen fell back to a naive GEMM solver because no workspace was
offered, and the stage will be many times slower than it should be.

```
MIOpen(HIP): Warning [IsEnoughWorkspace] Solver <GemmFwdRest>, workspace required: 547061760, provided ptr: 0 size: 0
```

## Rule 3: measure per stage, not per request

Whole-request timings hide which stage is broken. Prefer a harness that reports per-stage timings
(here `SamplingResult.stage_timings`) and compare each stage against the NVIDIA control. In this
workload the fp32 configuration made the DiT the dominant cost; after bf16 the decoder became
dominant, which changes what is worth optimizing next.

## Rule 4: strip repeated conditioning work before blaming the GPU

Re-encoding the same reference audio on every request cost 1.7–3.5 s per call. Encode once, cache
the latent, and pass the cached tensor. Cheap wins of this shape usually outrank kernel-level work.

## Rule 5: keep the CPU fallback measured, not assumed

For the conv-heavy decoder at 4.2 s of audio: GPU bf16 1.32 s, GPU fp32 2.19 s, CPU fp32 2.46 s
(8 threads). The CPU path is close enough that it is a legitimate fallback while a GPU kernel issue
is unresolved, and it avoids the MIOpen first-call cliff. Measure before choosing.

## Rule 6: pin the GPU by UUID

The host also exposes an iGPU (gfx1103 on this machine, 3 GB). Selecting by index can silently land
a workload on it. Use `ROCR_VISIBLE_DEVICES`/`HIP_VISIBLE_DEVICES` with the UUID from `rocminfo`,
and clear inherited `CUDA_VISIBLE_DEVICES`, `HSA_OVERRIDE_GFX_VERSION`, and `PYTORCH_ROCM_ARCH`.

## Known gaps on this host

- `rocm-smi`/`amd-smi` are not on `PATH`; read VRAM from `/sys/class/drm/card*/device/mem_info_vram_total`.
- CTranslate2 (faster-whisper) has no ROCm build. Run it on CPU with `compute_type="int8"`, or use a
  transformers-based ASR on the GPU when ASR is inside a training loop.
- torchcodec fails to load without matching FFmpeg shared libraries; read audio with `soundfile`
  instead of `torchaudio.load`.
- The Ubuntu installer may leave the root LV at 100 GB on a much larger disk. Check
  `lsblk`/`vgs` before concluding that a machine has no space for a ROCm environment.
