# Agave + QAT under Junction

This is a **source handoff** for the Junction+QAT configuration. It does not
bundle Junction, Caladan, the out-of-tree QAT driver, host firmware, or results.
Those are external, unpinned prerequisites; use versions compatible with your
host and debug version differences as needed. A successful fresh-clone build
and Junction run have **not yet been verified**.

## Source layout

Check out the following Git repositories side by side, using these directory
names (not Git submodules of Agave):

```
work/
  agave/                 # this fork
  solana-sdk/            # Liam's fork
  ed25519-dalek/         # Liam's fork
  ed25519-dalek-bip32/   # local modified checkout; publish a shareable fork
  curve25519-dalek/      # Liam's fork
  qat-shim/              # Liam's fork
```

`agave`, `solana-sdk`, `ed25519-dalek`, `curve25519-dalek`, and `qat-shim`
currently have remotes at `github.com:liam0215/<directory>.git`. The
`ed25519-dalek-bip32` checkout currently has an upstream-only remote; until
its changes are committed and shared, another developer cannot reproduce this
layout merely by cloning upstream. All local source changes must be published
to the appropriate Git remotes before this is an independent handoff.

The local QAT driver source directory is supplied via `QATLIB_SRC` and its
compiled `build` directory via `QATLIB_BUILD` (headers and `libqat_s.so`).
Junction's `junction_run`, the validator and bench config files, and their
hardware/NIC/permissions setup are supplied separately. Both Junction configs
need unique IPs; the validator config's `host_addr` must equal `MASTER_HOST`.
The validator config must enable QAT and asymmetric operations, as in the
current experiment. Do not copy existing validator/faucet JSON keys.

## Build and initialize

1. Copy `env.example` to `local.env` in this directory and fill in the paths
   and validator IP. `local.env` is ignored by Git. Use `bash build.sh` to
   build the six required Agave binaries with the `release-with-debug` profile.
   Rust versions are specified by the source repositories. The host needs
   the usual Agave build dependencies, libclang for bindgen, and the QAT
   driver's generated headers/libraries. Build in a fresh clone with ample
   free disk space; don't reuse binaries linked against a different QAT path.
2. In a **fresh clone with no `agave/config/`**, use `bash init-ledger.sh`.
   It generates fresh local faucet, identity, stake, and vote keypairs and a
   development genesis, refusing to overwrite any existing configuration.
   Unlike the original `multinode-demo/setup.sh`, it doesn't remove an
   existing ledger. It does not add SPL programs; pass additional genesis
   flags/paths if your workload needs them. Genesis runs on the host and no
   longer initializes a QAT session; the validator and benchmark run under
   Junction. If genesis fails after creating keys, inspect the partial
   ignored `config/` yourself before trying again—this script never deletes
   or overwrites it.
3. On the intended host, run `bash run-junction.sh validator` and, from a
   second shell, `bash run-junction.sh bench`. Both use the same generated
   identity and local ledger. The shell wrappers do not call `pkill`, modify
   Junction configs, remove ledgers, or write experimental CSVs. Capture logs
   and benchmark measurements outside the source tree.

The wrappers pass `--ld_path` and required variables through Junction's
`--env` options. They assume `/usr/bin/fish` is available to Junction and
the binary and key paths are visible at their host absolute paths. For
machine-specific CPU pinning/NUMA sweeps, use separate orchestration and
do not run the historical sweep scripts unchanged: they contain hardcoded
cores/IPs and destructive cleanup.

## Validation and limitations

Without launching Junction, `cargo metadata --offline --no-deps` from Agave
checks manifest layout. After building, verify the binary and `libqat_s.so`
linking, then validate signature behavior and a local transaction benchmark
under the actual Junction/QAT host. QAT memory-init and dedicated poll threads
have been removed; this configuration expects the poll-in-line integration.
No software-only or native-validator profile is supported by this handoff.

Results, plotting code, and a historical sweep script are collected separately
under `/data2/liam/agave-results/` on the original development machine. Do
not commit `agave/config/`, `target/`, `.so` build artifacts, logs, or results
into the Agave source repository.
