# gfx1201 ROCm user-space stack

## Contents

1. Repositories and validated role
2. Checkout
3. Preflight
4. AOTriton build and activation
5. vLLM inference
6. Transformers training
7. AITER evaluation
8. Validation and reporting

## Repositories and validated role

Use the private repositories on branch `gfx1201/all-improvements`:

| Component | Repository | Validated role |
| --- | --- | --- |
| AOTriton | `https://github.com/kizuna-intelligence/aotriton-gfx1201.git` | Packaged gfx1201 attention tuning/dispatch, PyTorch SDPA integration, and correctness-first fallback |
| vLLM | `https://github.com/kizuna-intelligence/vllm-gfx1201.git` | R9700 inference harness and evidence; enable the existing mainline `on_gfx1x` + `wvSplitK` dispatch |
| Transformers | `https://github.com/kizuna-intelligence/transformers-gfx1201.git` | BF16 training harness and adoption evidence for the gfx1201 AOTriton package |
| AITER | `https://github.com/kizuna-intelligence/aiter-gfx1201.git` | gfx1201 Flash Attention benchmark/regression harness; do not assume it is adopted for all training workloads |

Verified remote branch heads on 2026-07-22 were AOTriton `fe0275be`, vLLM `b77a599e`, Transformers `930e632e`, and AITER `9cd2b685`. Resolve and record the current full SHA at checkout time; do not silently depend on a moving branch head.

The measured improvements are user-space changes. No amdgpu kernel driver, firmware, or host ROCm package modification is part of this stack.

## Checkout

Clone only the components required by the workload:

```bash
git clone --branch gfx1201/all-improvements --single-branch \
  https://github.com/kizuna-intelligence/aotriton-gfx1201.git
git clone --branch gfx1201/all-improvements --single-branch \
  https://github.com/kizuna-intelligence/vllm-gfx1201.git
git clone --branch gfx1201/all-improvements --single-branch \
  https://github.com/kizuna-intelligence/transformers-gfx1201.git
git clone --branch gfx1201/all-improvements --single-branch \
  https://github.com/kizuna-intelligence/aiter-gfx1201.git
```

After checkout, record provenance with `git rev-parse HEAD` and inspect `MIRROR_MANIFEST.md` before using a component.

## Preflight

Confirm the visible hardware and software before a build or run:

```bash
rocminfo | rg 'Name:|Marketing Name:|Uuid:'
amd-smi process
python - <<'PY'
import torch
print("torch", torch.__version__, "hip", torch.version.hip)
print("available", torch.cuda.is_available(), "count", torch.cuda.device_count())
if torch.cuda.is_available():
    p = torch.cuda.get_device_properties(0)
    print(torch.cuda.get_device_name(0), getattr(p, "gcnArchName", None), getattr(p, "uuid", None))
PY
```

Use `ROCR_VISIBLE_DEVICES=<GPU-UUID>` and clear conflicting inherited visibility variables when needed. Acquire the worker's established GPU lock before work. The R9700 experiment scripts use `/tmp/cyborgy-r9700-gpu1.lock`; do not assume that lock name or the recorded experiment UUID on another worker.

## AOTriton build and activation

The AOTriton fork supplies these reusable scripts:

- `tools/build_gfx1201_for_torch.sh SOURCE_DIR BUILD_DIR INSTALL_DIR`
- `tools/run_gfx1201_remote_build.sh SOURCE_DIR BUILD_DIR INSTALL_DIR TRITON_CACHE_DIR LOG`
- `tools/run_gfx1201_remote_matrix.sh` for the validated build/test matrix
- `tools/gfx1201_tuning.py`, `tools/gfx1201_db.py`, and `tools/gfx1201_fallback.py` for tuning, database generation, replay, and fallback checks

`run_gfx1201_remote_build.sh` is host-specific example automation: inspect its image, UUID, paths, and lock before use. For a compatible ROCm/PyTorch container, invoke the generic build script with explicit source, build, and install directories:

```bash
export AOTRITON_CI_SUPPLIED_SHA1="$(git -C aotriton-gfx1201 rev-parse HEAD)"
export AOTRITON_GIT_TREESHA1="$(git -C aotriton-gfx1201 rev-parse HEAD^{tree})"
aotriton-gfx1201/tools/build_gfx1201_for_torch.sh \
  "$PWD/aotriton-gfx1201" "$PWD/aot-build" "$PWD/aot-install"
```

The build must target `gfx1201`/`gfx1201_mod0`, contain real kernel image packs, and produce `gfx1201-build-manifest.json`. Activate it for a containerized workload by mounting the install directory read-only and prepending its library directory:

```bash
docker run --rm --device=/dev/kfd --device=/dev/dri --ipc=host \
  -v "$PWD/aot-install:/aotriton-install:ro" \
  -e "ROCR_VISIBLE_DEVICES=<GPU-UUID>" \
  -e LD_LIBRARY_PATH=/aotriton-install/lib:/opt/rocm/lib:/opt/rocm/lib64 \
  <validated-rocm-pytorch-image> <workload-command>
```

Preserve the packaged PyTorch default fallback. The exact BF16/D128/S8192 causal backward case is intentionally routed to default PyTorch SDPA after an input-seed correctness failure in the source fallback.

## vLLM inference

Use the vLLM fork and enable the existing gfx1x skinny-GEMM route:

```bash
export VLLM_ROCM_USE_SKINNY_GEMM=1
```

Build/install vLLM using the repository's ROCm instructions for the pinned revision. Verify at runtime that `rocm_unquantized_gemm_impl` contains the `on_gfx1x` route and that the loaded `_rocm_C.abi3.so` belongs to the same build. Do not mix a new Python tree with a stale extension.

Use these repository scripts and evidence for reproduction:

- `scripts/e8aed82c/run_r9700_matrix.sh`
- `benchmarks/e2e/benchmark_vllm_bf16_e2e.py`
- `benchmarks/kernels/benchmark_rocm_decode_gemv.py`
- `benchmarks/kernels/benchmark_rocm_decode_ops.py`
- `results/90ce7a84/REPORT.md` and `results/e8aed82c/REPORT.md`

The adopted result is the existing mainline `on_gfx1x` + `wvSplitK` path. The private fork adds validation, tests, and evidence; do not claim a new private kernel patch. The AOTriton overlay improved the measured vLLM E2E case by only about 0.17% and was not adopted for that inference recipe.

## Transformers training

Use the Transformers fork as the workload source and the packaged AOTriton install for the adopted BF16 training path. For a source checkout, either install it in an isolated environment or point `PYTHONPATH` at `src`:

```bash
export PYTHONPATH="$PWD/transformers-gfx1201/src${PYTHONPATH:+:$PYTHONPATH}"
```

Reproduce the one-variable matrix with:

- `scripts/b3c19b75/run_r9700_matrix.sh`
- `scripts/b3c19b75/run_training_matrix_remote.sh`
- `benchmarks/e2e/benchmark_transformers_bf16_train_e2e.py`
- `results/b3c19b75/REPORT.md`

The measured TinyLlama BF16 training recipe adopted the gfx1201 AOTriton package at about +7.6% median tokens/s. AITER was a non-adopted condition for that recipe.

## AITER evaluation

Use the AITER fork when the workload specifically needs its gfx1201 Flash Attention harness or when testing a new AITER candidate. Inspect `MIRROR_MANIFEST.md` and run the relevant tests under the same correctness, lock, and provenance rules. Do not replace the adopted AOTriton training path with AITER solely because the fork exists.

## Validation and reporting

For every modified-stack result:

1. Keep a stock control with identical model revision, seed, dtype, shapes, batch, warm-up, iterations, and visible GPU.
2. Confirm finite output and gradients where applicable, then enforce the workload's numerical tolerance.
3. Record the component SHA, container digest, PyTorch/HIP versions, GPU architecture/UUID, environment overrides, and loaded library hashes.
4. Run at least three timed repetitions after warm-up and report median; keep raw JSON when the repository harness emits it.
5. State adoption per workload. A component may be adopted for Transformers training and non-adopted for vLLM inference.
6. Describe failures and safe fallbacks without hiding them. Never generalize an R9700/gfx1201 result to every AMD GPU without new evidence.
