# Benchmark results

* Date: 2026-11-23T16:16:42.104509500+00:00
* Build: release (`cargo run -p ma-bench --release`)
* Machine: Intel64 Family 6 Model 141 Stepping 1, GenuineIntel — 16 logical cores, 16.9 GB RAM, windows
* Storage: local SSD temp directory; state database SQLite (WAL, synchronous=FULL)
* Workload: CSV source → JSON Lines destination through the full plan → approve → execute → verify path; batch size 5000 (2000 for interrupt test); per-batch fsync + atomic SQLite checkpoint; id map persisted for every record.

| scenario | records | execute (s) | records/s | verify (s) | verified rec/s | peak RSS (MB, process high-water) | CPU util. | result |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| CSV→JSONL, 7 transformed fields, 10000 | 10000 | 0.16 | 62211 | 0.26 | 38897 | 56 | 7% | Completed and verified |
| CSV→JSONL, 7 transformed fields, 100000 | 100000 | 1.53 | 65434 | 2.33 | 42892 | 91 | 7% | Completed and verified |
| CSV→JSONL, 7 transformed fields, 1000000 | 1000000 | 25.68 | 38940 | 25.33 | 39484 | 104 | 5% | Completed and verified |
| relationships: 20000 parents + 200000 children | 220000 | 4.49 | 48987 | 8.17 | 26922 | 104 | 7% | Completed and verified |
| 3 concurrent jobs x 100000 | 300000 | 4.48 | 67021 | 6.99 | 42943 | 203 | 7% | all verified |
| pause after 5 batches + resume, 200000 | 200000 | 3.06 | 65312 | - | - | 203 | - | resumed, no duplicates/gaps |
| transform runtime only (in-memory, 1 thread) | 200000 | 0.47 | 424178 | - | - | 317 | - | n/a |

Peak RSS is the process high-water mark at the time the row was recorded (cumulative across earlier rows).

## Run-to-run variance (same machine, same build, consecutive runs)

| scenario | run A execute / verify (s) | run B execute / verify (s) |
|---|---|---|
| 100,000 records | 1.66 / **14.78** | 1.53 / 2.33 |
| 1,000,000 records | **14.71** / 24.40 | **25.68** / 25.33 |
| 3 concurrent jobs × 100,000 | 4.51 / 7.32 | 4.48 / — |

Run B is the table above. The spread comes from storage I/O (every batch fsyncs the output file
and commits the SQLite state with `synchronous=FULL`; CPU utilization stays at 5–8 % of 16 cores,
so the pipeline is I/O-bound, not CPU-bound). The single-threaded transformation runtime alone
runs at ~420,000 records/s, about 6–10× the end-to-end rate. Treat the numbers as indicative for
this laptop; they are not guarantees.

### Interpretation
* Memory is bounded: peak resident memory grows from ~56 MB (10k) to ~104 MB (1M) — streaming,
  not proportional to dataset size.
* Throughput is limited by durable checkpointing, by design (correctness over speed). Larger
  batches or a faster disk raise throughput; relaxing durability is not offered.
* Verification re-reads the source and destination completely and recomputes every expected
  record (full content checksum), so it costs about as much as the migration itself.
