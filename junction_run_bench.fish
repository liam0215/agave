set -lx LD_LIBRARY_PATH /data/liam/qatlib/build/lib $LD_LIBRARY_PATH

/data2/liam/agave/target/release-with-debug/solana-bench-tps --url "http://192.168.120.7:8899" --entrypoint "192.168.120.7:8001" --faucet "192.168.120.7:9900" --duration 50 --tx-count 50000 --thread-batch-sleep-ms 0 --bind-address 127.0.0.1 --client-node-id /data2/liam/agave/net/../config/bootstrap-validator/identity.json
# --block-data-file /data/liam/junction-arkose/build/junction/block_data.txt
# --transaction-data-file /data/liam/junction-arkose/build/junction/txn_data.txt

# cat /data/liam/junction-arkose/build/junction/txn_data.txt

# cat /data/liam/junction-arkose/build/junction/block_data.txt

