#!/usr/bin/fish

set -l bin "$AGAVE_ROOT/target/release-with-debug"
set -l identity "$AGAVE_ROOT/config/bootstrap-validator/identity.json"
set -lx LD_LIBRARY_PATH $QATLIB_BUILD $LD_LIBRARY_PATH

if not test -x "$bin/solana-bench-tps" -a -r "$identity"
    echo "Missing benchmark binary or local identity: run build.sh and init-ledger.sh" >&2
    exit 1
end

"$bin/solana-bench-tps" \
    --url "http://$MASTER_HOST:8899" --entrypoint "$MASTER_HOST:8001" \
    --faucet "$MASTER_HOST:9900" --duration "$BENCH_DURATION" \
    --tx-count "$BENCH_TX_COUNT" --thread-batch-sleep-ms 0 \
    --bind-address 0.0.0.0 --client-node-id "$identity"
