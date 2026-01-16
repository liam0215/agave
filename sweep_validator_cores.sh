#!/usr/bin/env bash
set -euo pipefail

# === Config you likely want to tweak ===
# Full pool of validator cores (as you provided)
VALIDATOR_POOL=(1 57 3 59 5 61 7 63 9 65 11 67 13 69 15 71 17 73 19 75 21 77 23 79 25 81 27 83 29 85 31 87 33 89 35)

# Fixed bench cores (as you provided) — keep constant
BENCH_CORES="35,37,39,41,43,45,47,49,51,53,91,93,95,97,99,101,103,105,107,109"

# How many validator cores to try each run (in order).
# Feel free to edit this list; it must not exceed ${#VALIDATOR_POOL[@]}.
# SIZES=(34 32 30 28 26 24 22 20 18 16 14 12)
SIZES=(24)

# NUMA node for memory binding
MEM_NODE=1

# Paths to your scripts
RUN_VALIDATOR="./run_val.sh"
RUN_BENCH="./run_bench.sh"

# Output results file
OUT_CSV="validator_sweep_results.csv"

# Optional grace/settle times (seconds)
VALIDATOR_STARTUP_WAIT=90   # time to let validator boot before starting bench
VALIDATOR_SHUTDOWN_WAIT=20  # grace period after sending SIGINT

# === End config ===

# Ensure dependencies exist
command -v numactl >/dev/null || { echo "numactl not found in PATH"; exit 1; }
[[ -x "$RUN_VALIDATOR" ]] || { echo "Validator script not executable: $RUN_VALIDATOR"; exit 1; }
[[ -x "$RUN_BENCH"     ]] || { echo "Bench script not executable: $RUN_BENCH"; exit 1; }

# Write CSV header if file doesn't exist
if [[ ! -f "$OUT_CSV" ]]; then
  echo "timestamp,size,validator_core_list,bench_cores,avg_tps" > "$OUT_CSV"
fi

# Cleanup handler to kill background validator on exit
VALIDATOR_PID=""
cleanup() {
  if [[ -n "${VALIDATOR_PID}" ]] && kill -0 "$VALIDATOR_PID" 2>/dev/null; then
    echo "Stopping validator (pid=$VALIDATOR_PID)..."
    kill -INT "$VALIDATOR_PID" 2>/dev/null || true
    sleep "$VALIDATOR_SHUTDOWN_WAIT" || true
    kill -TERM "$VALIDATOR_PID" 2>/dev/null || true
    pkill "validator" 2>/dev/null || true
    pkill "faucet" 2>/dev/null || true
  fi
}
trap cleanup EXIT

for size in "${SIZES[@]}"; do
for i in {1..3}; do
  rm -r config/ || true
  mkdir -p config/
  NDEBUG=1 ./multinode-demo/setup.sh
  if (( size > ${#VALIDATOR_POOL[@]} )); then
    echo "Requested size $size exceeds validator pool size ${#VALIDATOR_POOL[@]} — skipping."
    continue
  fi

  # Build the core list string for this run (first N cores from the pool)
  cores_csv=$(IFS=,; echo "${VALIDATOR_POOL[*]:0:size}")

  echo "=== Running with validator cores: [$cores_csv] (size=$size), bench cores: [$BENCH_CORES] ==="

  # Start validator in background under numactl
  numactl -C "$cores_csv" --membind="$MEM_NODE" "$RUN_VALIDATOR" >"validator_${size}.log" 2>&1 &
  VALIDATOR_PID=$!

  # Give it a moment to start up
  sleep "$VALIDATOR_STARTUP_WAIT"

  bench_log="bench_${size}.log"

  $RUN_BENCH &

  sleep "$VALIDATOR_SHUTDOWN_WAIT"
  pkill "bench" 2>/dev/null || true

  $RUN_BENCH &

  sleep "$VALIDATOR_SHUTDOWN_WAIT"
  pkill "bench" 2>/dev/null || true
  sleep "$VALIDATOR_SHUTDOWN_WAIT"
  
  rm -f "$bench_log"
  # Run bench with line-buffered stdout/stderr, capture BOTH to tee
  if ! stdbuf -oL -eL \
       numactl -C "$BENCH_CORES" --membind="$MEM_NODE" "$RUN_BENCH" \
       2>&1 | tee "$bench_log"; then
    echo "Bench run failed for size=$size (see $bench_log)."
  fi

  # Parse "Average TPS: 98965.805" (strip any thousands commas just in case)
  avg_tps=$(grep -Eo 'Average TPS: *[0-9][0-9.,]*' "$bench_log" \
            | awk '{print $3}' | tr -d ',' | tail -n1 || true)
  # Append to CSV
  ts=$(date --iso-8601=seconds)
  echo "$ts,$size,\"$cores_csv\",\"$BENCH_CORES\",$avg_tps" >> "$OUT_CSV"
  echo "Recorded: size=$size avg_tps=$avg_tps"

  # Stop validator cleanly
  if kill -0 "$VALIDATOR_PID" 2>/dev/null; then
    kill -INT "$VALIDATOR_PID" 2>/dev/null || true
    pkill "validator" 2>/dev/null || true
    pkill "faucet" 2>/dev/null || true
    pkill "bench" 2>/dev/null || true
    sleep "$VALIDATOR_SHUTDOWN_WAIT" || true
    pkill "validator" 2>/dev/null || true
    pkill "faucet" 2>/dev/null || true
    pkill "bench" 2>/dev/null || true
    kill -TERM "$VALIDATOR_PID" 2>/dev/null || true
  fi
  sudo pkill "validator" 2>/dev/null || true
  sudo pkill "faucet" 2>/dev/null || true
  sudo pkill "bench" 2>/dev/null || true
  VALIDATOR_PID=""
done
done

echo "Done. Results in: $OUT_CSV"
