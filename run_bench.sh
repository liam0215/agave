export LD_LIBRARY_PATH="/data/liam/qatlib/build/lib:$LD_LIBRARY_PATH"
/data2/liam/agave/target/release-with-debug/solana-bench-tps --url "http://127.0.0.1:8899" --entrypoint "127.0.0.1:8001" --faucet "127.0.0.1:9900" --duration 90 --tx-count 50000 --thread-batch-sleep-ms 0 --bind-address 127.0.0.1 --client-node-id /data2/liam/agave/net/../config/bootstrap-validator/identity.json
