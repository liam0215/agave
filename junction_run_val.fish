set -lx LD_LIBRARY_PATH /data/liam/qatlib/build/lib $LD_LIBRARY_PATH
set MASTER 192.168.120.7

pkill "faucet"
pkill "validator"
/data2/liam/agave/target/release-with-debug/solana-faucet --keypair /data2/liam/agave/config/faucet.json &
sleep 5

# export SOLANA_METRICS_CONFIG="host=127.0.0.1:8125"

export RUST_LOG=\
solana=info,solana_core::sigverify_stage=info
# solana_core=info
# solana_sigverify_stage=debug
# solana_sigverify_stage=debug
# solana=debug
# solana_tpu=info
# solana_streamer=debug,\
# solana_banking_stage=debug,\
# solana_runtime::bank=debug,\
# solana_cost_model=debug,\
# solana_poh=debug

/data2/liam/agave/target/release-with-debug/agave-validator --require-tower --ledger /data2/liam/agave/config/bootstrap-validator --rpc-port 8899 --snapshot-interval-slots 200 --no-incremental-snapshots --identity /data2/liam/agave/config/bootstrap-validator/identity.json --vote-account /data2/liam/agave/config/bootstrap-validator/vote-account.json --rpc-faucet-address $MASTER:9900 --no-poh-speed-test --no-os-network-limits-test --no-wait-for-vote-to-start-leader --full-rpc-api --allow-private-addr --rocksdb-ledger-compression lz4 --gossip-port 8001 --public-tpu-address $MASTER:8003 --log -
