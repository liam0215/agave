pkill "faucet"
pkill "validator"
export LD_LIBRARY_PATH="/data/liam/qatlib/build/lib:$LD_LIBRARY_PATH"
/data2/liam/agave/target/release-with-debug/solana-faucet --keypair /data2/liam/agave/config/faucet.json &
sleep 5

# export RUST_LOG=\
# solana_tpu=debug,\
# solana_streamer=debug,\
# solana_sigverify=debug,\
# solana_banking_stage=debug,\
# solana_runtime::bank=debug,\
# solana_cost_model=debug,\
# solana_poh=debug,\
# solana_metrics=debug

/data2/liam/agave/target/release-with-debug/agave-validator --require-tower --ledger /data2/liam/agave/config/bootstrap-validator --rpc-port 8899 --snapshot-interval-slots 200 --no-incremental-snapshots --identity /data2/liam/agave/config/bootstrap-validator/identity.json --vote-account /data2/liam/agave/config/bootstrap-validator/vote-account.json --rpc-faucet-address 127.0.0.1:9900 --no-poh-speed-test --no-os-network-limits-test --no-wait-for-vote-to-start-leader --full-rpc-api --allow-private-addr --rocksdb-ledger-compression lz4 --gossip-port 8001

