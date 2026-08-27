# Thermal guard for sustained gfx1201 (R9700) runs

Read this before starting any AMD GPU job that runs unattended for more than a few minutes:
long diffusion sampling, training, or a batch generation sweep. A short benchmark does not
need it.

## Measured behaviour

Radeon AI PRO R9700 (gfx1201) under sustained image-diffusion sampling, observed on two
separate hosts on 2026-08-15/16:

| Host | Workload | Junction (hotspot) | Notes |
| --- | --- | ---: | --- |
| dev-computer-2 GPU1 | Z-Image BF16, 50 steps, 1024x1024 | 110-112 C | fan 82% / 4,200 RPM, still climbing |
| dev-computer-2 GPU0 | same | 109-111 C | VAE decode stage peaked highest |
| second host, single R9700 | SDXL 28 steps, 1024x1024 | 106 C within ~3 min | idle 33-34 C before the run |

Junction reaches the hardware slowdown boundary within minutes and stays there. Edge and
memory temperatures stay far lower (edge ~53 C, memory ~78 C while junction is 112 C), so an
edge-temperature check will not see the problem. **Read `HOTSPOT`, not `EDGE`.**

Letting the driver handle it is not sufficient for a multi-hour run: the card then sits pinned
at the throttle boundary for the whole job.

## Practice

Run the job under a guard process that suspends and resumes it by signal, instead of letting
the card sit at the boundary.

- Stop at hotspot **>= 105 C**, resume at **<= 70 C**. The wide hysteresis is deliberate; a
  narrow band produces continuous stop/start churn with no cooling benefit.
- Poll every 2-3 s. Take the first `HOTSPOT:` value from `amd-smi metric --gpu <N>`.
- `kill -STOP` / `kill -CONT` the **exact PID this task started**. Never match by process name;
  that violates the runtime safeguard against broad kills and can suspend another worker's job.
- Before each signal, re-read `/proc/<pid>/cmdline` and confirm it still contains the expected
  run identifier. A recycled PID must not be signalled.
- Log every pause and resume with the temperature. Throughput drops roughly by half in this
  regime, and without the log that reads as a performance regression.

### The guard must outlive its launcher

- Always `kill -CONT` on guard exit (`trap ... EXIT TERM INT HUP`).
- **Start the guard detached: `setsid nohup guard.sh <pid> <tag> &`.** Do not spawn it as a
  child of the driver or orchestrator that started the job.

A child guard dies whenever that driver is stopped or replaced, and the `trap` does not save
you because the guard is killed along with it. Observed on 2026-08-16: redundant driver shells
were cleaned up while both jobs happened to be inside a 105 C pause, their child guards died
with them, and two jobs sat in state `T` indefinitely with the GPU idle at 53 C. **The visible
symptom was the fans going quiet — no error, no log line, no failed exit code.**

### Detect the frozen state

A job in process state `T` while GPU utilisation is ~0 and the hotspot is below the resume
threshold is stuck, not paused:

```bash
ps -o pid=,stat= -p "$GEN"      # T = stopped
rocm-smi --showuse --showtemp   # ~0% and cool => nothing will resume it
```

Recover with `kill -CONT "$GEN"` and attach a detached guard. Include this check in any status
report for a long AMD run; "still running" is not the same as "still computing".

### Do not stack jobs on one card

Two ~15 GB jobs fit in a 32 GB R9700 but leave no headroom for a third, and they double the
number of processes a single guard failure can freeze. Serialise instead.

## Reusable guard

Save as `guard.sh` and launch with `setsid nohup ./guard.sh <target-pid> <tag> &`.

```bash
#!/bin/bash
# guard.sh <target-pid> <tag> — thermal guard for one AMD GPU job
set -u
GEN=$1
TAG=$2
GPU_INDEX=${GPU_INDEX:-0}
RUN_ID=${RUN_ID:-}
LOG="${GUARD_LOG:-guard_${TAG}.log}"
paused=0

trap 'kill -CONT "$GEN" 2>/dev/null || true; echo "$(date -Is) guard_exit_cont" >> "$LOG"' EXIT TERM INT HUP
echo "$(date -Is) guard_start pid=$GEN tag=$TAG" >> "$LOG"

while kill -0 "$GEN" 2>/dev/null; do
  cmd=$(tr '\0' ' ' < "/proc/$GEN/cmdline" 2>/dev/null || true)
  if [ -n "$RUN_ID" ]; then
    case "$cmd" in *"$RUN_ID"*) ;; *) echo "$(date -Is) identity_mismatch" >> "$LOG"; exit 1;; esac
  fi
  t=$(amd-smi metric --gpu "$GPU_INDEX" 2>/dev/null | awk '/HOTSPOT:/{print $2; exit}')
  case "$t" in ''|*[!0-9]*) sleep 3; continue;; esac
  if [ "$paused" -eq 0 ] && [ "$t" -ge 105 ]; then
    kill -STOP "$GEN"; paused=1; echo "$(date -Is) pause hotspot=$t" >> "$LOG"
  elif [ "$paused" -eq 1 ] && [ "$t" -le 70 ]; then
    kill -CONT "$GEN"; paused=0; echo "$(date -Is) resume hotspot=$t" >> "$LOG"
  fi
  sleep 3
done
echo "$(date -Is) target_exited" >> "$LOG"
```

Pair the guard with a resumable job. Intermittent operation only works if the workload writes
each unit of output atomically and skips completed units on restart. A crash between writing
an artifact and writing its sidecar leaves a half-pair that a strict resume check refuses,
which stalls the next attempt until the orphan is removed — write both under one atomic
rename, or clean orphans before resuming.

## Do not

- Do not change GPU clocks, power limits, or fan curves to work around the temperature. That is
  a host configuration change and needs explicit authorization.
- Do not raise the 105 C threshold to make a run finish faster.
- Do not treat a thermal pause as a failure and restart the job; it resumes by itself.
