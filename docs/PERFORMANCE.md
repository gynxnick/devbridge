# Performance — measured results

Measured 2026-10-26 on a MacBook Air (Apple Silicon, macOS 26.6), release build, both
"devices" on the same machine over loopback TLS. Real LAN throughput will be lower
(network-bound). Re-run with:

```sh
cargo test --release -p devbridge-sync --test perf -- --ignored --nocapture --test-threads=1
```

| Measurement | Result | Spec target |
|---|---|---|
| Initial index of 10,000 small files (scan + hash + record versions) | 0.32 s | Large projects handled incrementally ✔ |
| Re-scan of the same 10,000 unchanged files | 0.041 s, 0 versions | Unchanged files not re-processed ✔ |
| Re-index one changed path | 0.7 ms | — |
| BLAKE3 hash of a 256 MiB file | 0.13 s (≈1,970 MiB/s), constant memory (256 KiB buffer) | Large binaries not loaded into memory ✔ |
| Small edit: write on A → content on B's disk (median of 10) | 384 ms (max 388 ms), including the 300 ms watcher debounce | "Begin promptly" ✔ |
| 200 MiB file A → B (hash, transfer in 1 MiB chunks, verify, atomic rename) | 0.72 s (≈276 MiB/s) | Bounded memory: chunk size × concurrency ✔ |
| Idle app process (debug build, 1 small project, no peers) | 0.3–0.5 % CPU, ~70 MB RSS; 1.0 s total CPU time over ~10 min | Low idle CPU ✔ |

Not yet measured: real two-machine Wi-Fi/Ethernet throughput, Windows NTFS indexing speed,
and projects above 100k files. Those need the cross-machine checklist in TESTING.md.
