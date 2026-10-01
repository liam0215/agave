#!/usr/bin/fish

set -l bin "$AGAVE_ROOT/target/release-with-debug"
set -l ledger "$AGAVE_ROOT/config/bootstrap-validator"
set -lx LD_LIBRARY_PATH $QATLIB_BUILD $LD_LIBRARY_PATH

for file in "$bin/solana-faucet" "$bin/agave-validator" "$AGAVE_ROOT/config/faucet.json" "$ledger/identity.json" "$ledger/vote-account.json" "$ledger/genesis.bin"
    if not test -e "$file"
        echo "Missing $file" >&2
        exit 1
    end
end

set -g FAUCET_PID ''
function stop_faucet --on-event fish_exit
    if test -n "$FAUCET_PID"
        kill $FAUCET_PID 2>/dev/null
    end
end

"$bin/solana-faucet" --keypair "$AGAVE_ROOT/config/faucet.json" &
set -g FAUCET_PID $last_pid
sleep 5

"$bin/agave-validator" --require-tower --ledger "$ledger" \
    --rpc-port 8899 --snapshot-interval-slots 200 --no-incremental-snapshots \
    --identity "$ledger/identity.json" --vote-account "$ledger/vote-account.json" \
    --rpc-faucet-address "$MASTER_HOST:9900" --no-poh-speed-test \
    --no-os-network-limits-test --no-wait-for-vote-to-start-leader \
    --full-rpc-api --allow-private-addr --rocksdb-ledger-compression lz4 \
    --gossip-port 8001 --public-tpu-address "$MASTER_HOST:8003" --log -
