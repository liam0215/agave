#!/usr/bin/env bash
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
root=$(cd -- "$here/../.." && pwd)
if [[ -f $here/local.env ]]; then
  # shellcheck source=/dev/null
  source "$here/local.env"
fi

bin="$root/target/release-with-debug"
for program in solana-keygen solana-genesis; do
  [[ -x $bin/$program ]] || { echo "Build $program first: $here/build.sh" >&2; exit 1; }
done

# Never overwrite an existing validator identity, vote account, or ledger.
if [[ -e $root/config ]]; then
  echo "Refusing to replace existing $root/config; use a fresh clone or move it aside yourself" >&2
  exit 1
fi

umask 077
# The genesis binary no longer initializes QAT. Keep the driver library in the
# loader path because other linked fork dependencies may still need it.
if [[ -n ${QATLIB_SRC:-} ]]; then
  export QATLIB_BUILD="${QATLIB_BUILD:-$QATLIB_SRC/build}"
  export LD_LIBRARY_PATH="$QATLIB_BUILD${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
fi
mkdir -p "$root/config/bootstrap-validator"
keygen="$bin/solana-keygen"
ledger="$root/config/bootstrap-validator"
"$keygen" new --no-passphrase --silent --outfile "$root/config/faucet.json"
"$keygen" new --no-passphrase --silent --outfile "$ledger/identity.json"
"$keygen" new --no-passphrase --silent --outfile "$ledger/stake-account.json"
"$keygen" new --no-passphrase --silent --outfile "$ledger/vote-account.json"
cd "$root"
"$bin/solana-genesis" \
  --ledger "$ledger" \
  --faucet-pubkey "$root/config/faucet.json" \
  --faucet-lamports 500000000000000000 \
  --hashes-per-tick auto \
  --cluster-type development \
  --enable-warmup-epochs \
  --bootstrap-validator "$ledger/identity.json" "$ledger/vote-account.json" "$ledger/stake-account.json" \
  "$@"
