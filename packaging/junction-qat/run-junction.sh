#!/usr/bin/env bash
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
root=$(cd -- "$here/../.." && pwd)
if [[ -f $here/local.env ]]; then
  # shellcheck source=/dev/null
  source "$here/local.env"
fi

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 validator|bench (run separately, with two Junction configs)" >&2
  exit 2
fi
: "${JUNCTION_RUN:?Set JUNCTION_RUN in local.env}"
: "${QATLIB_SRC:?Set QATLIB_SRC in local.env}"
: "${MASTER_HOST:?Set MASTER_HOST to the validator host_addr in local.env}"
export QATLIB_BUILD="${QATLIB_BUILD:-$QATLIB_SRC/build}"
ld_path="${JUNCTION_LD_PATH:-$QATLIB_BUILD}"
[[ -x $JUNCTION_RUN && -f $ld_path/libqat_s.so && -x /usr/bin/fish ]] || {
  echo "Missing junction_run, libqat_s.so, or /usr/bin/fish" >&2
  exit 1
}

case "$1" in
  validator)
    : "${JUNCTION_VALIDATOR_CONFIG:?Set JUNCTION_VALIDATOR_CONFIG in local.env}"
    config=$JUNCTION_VALIDATOR_CONFIG
    script=$here/validator.fish
    executable=$root/target/release-with-debug/agave-validator
    [[ -r $root/config/bootstrap-validator/genesis.bin && -r $root/config/faucet.json ]] || {
      echo "Missing local genesis/keys. See init-ledger.sh (never commit config/)." >&2
      exit 1
    }
    ;;
  bench)
    : "${JUNCTION_BENCH_CONFIG:?Set JUNCTION_BENCH_CONFIG in local.env}"
    config=$JUNCTION_BENCH_CONFIG
    script=$here/bench.fish
    executable=$root/target/release-with-debug/solana-bench-tps
    [[ -r $root/config/bootstrap-validator/identity.json ]] || {
      echo "Missing local validator identity; generate a ledger first." >&2
      exit 1
    }
    ;;
  *) echo "Choose validator or bench" >&2; exit 2 ;;
esac
[[ -f $config && -x $executable ]] || { echo "Missing Junction config or $executable" >&2; exit 1; }

# --env explicitly passes the variables needed inside Junction; sudo need not
# preserve the caller's environment. The two configs must have distinct IPs.
args=(--env "AGAVE_ROOT=$root" --env "QATLIB_BUILD=$QATLIB_BUILD"
      --env "MASTER_HOST=$MASTER_HOST" --env "BENCH_DURATION=${BENCH_DURATION:-50}"
      --env "BENCH_TX_COUNT=${BENCH_TX_COUNT:-50000}")
for variable in SOLANA_BANKING_THREADS SOL_SIGVERIFY_THREADS RUST_LOG; do
  if [[ -v $variable ]]; then
    args+=(--env "$variable=${!variable}")
  fi
done
exec sudo "$JUNCTION_RUN" "$config" "${args[@]}" --ld_path "$ld_path" -- \
  /usr/bin/fish "$script"
