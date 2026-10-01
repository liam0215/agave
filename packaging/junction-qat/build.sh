#!/usr/bin/env bash
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
root=$(cd -- "$here/../.." && pwd)
if [[ -f $here/local.env ]]; then
  # shellcheck source=/dev/null
  source "$here/local.env"
fi

: "${QATLIB_SRC:?Set QATLIB_SRC to the QAT driver source directory in local.env}"
export QATLIB_SRC
export QATLIB_BUILD="${QATLIB_BUILD:-$QATLIB_SRC/build}"
[[ -d $QATLIB_SRC && -d $QATLIB_BUILD/include && -f $QATLIB_BUILD/libqat_s.so ]] || {
  echo "QAT driver source, generated headers, or libqat_s.so not found" >&2
  exit 1
}

cd "$root"
cargo build --locked --profile release-with-debug \
  -p agave-validator -p solana-faucet -p solana-genesis \
  -p solana-keygen -p solana-bench-tps -p solana-bench-tps-rate
