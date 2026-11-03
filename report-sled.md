# Acts million-run stress report

- Generated at (Unix seconds): 1793665280
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 1708.68s
- Measured throughput: 58.52 workflows/s
- Peak RSS: 1.29 GiB
- Min system available memory: 13.35 GiB
- Process CPU: peak 354.3% / sample mean 315.9%
- System CPU: peak 85.3% / sample mean 77.7%
- Max storage used: 1.11 GiB
- Peak process rows: 1069 (active 128)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.001s

## Test configuration

- Workload: workflow: init + variable while-loop + expression evaluation + two-way branch + finalize
- Host: linux / x86_64 (4 logical CPUs)
- Concurrency stages: [16, 32, 64, 128]
- Warmup workflows: 100
- Steady-state soak: 0s
- Cleanup drain wait: 300s
- Scheduler workers: 4
- Store writer workers: 4
- Store shards: 4
- Cache cap: 320
- Scheduler queue cap: 4096
- Store writer queue cap: 4096
- Metric sample interval: 1000ms (each status sample also waits for sysinfo's CPU delta window)
- Per-workflow wait timeout: 120s
- Engine version: 0.1.0
- Store backend: sled (target/stress-results/sled-store, 4 shards, 50ms flush)
- Error-rate gate: 0.000%

Backend selected by --store; connection URLs are not copied into this report; closed-loop load; sampled peaks may understate instantaneous peaks. Run a release build on the target machine.

## Per-phase results

| Phase | Concurrency | Attempted | Completed | Errored | Terminated | Throughput/s | Error rate | Elapsed/s |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| warmup | 16 | 100 | 100 | 0 | 0 | 62.99 | 0.0000% | 1.59 |

**warmup start API latency (ms)**: p50 6.012 / p90 16.416 / p95 19.624 / p99 20.679 / p99.9 21.119 / max 21.119 / mean 7.840 / stddev 4.730
**warmup end-to-end latency (ms)**: p50 204.464 / p90 421.111 / p95 472.138 / p99 513.002 / p99.9 530.913 / max 530.913 / mean 236.013 / stddev 128.974

| load-1/4-c16 | 16 | 25000 | 25000 | 0 | 0 | 59.83 | 0.0000% | 417.85 |

**load-1/4-c16 start API latency (ms)**: p50 6.309 / p90 8.642 / p95 9.393 / p99 11.003 / p99.9 35.331 / max 184.828 / mean 6.549 / stddev 3.586
**load-1/4-c16 end-to-end latency (ms)**: p50 221.405 / p90 547.168 / p95 643.726 / p99 802.057 / p99.9 978.184 / max 1043.426 / mean 267.368 / stddev 189.195

| load-2/4-c32 | 32 | 25000 | 25000 | 0 | 0 | 57.76 | 0.0000% | 432.82 |

**load-2/4-c32 start API latency (ms)**: p50 7.146 / p90 9.510 / p95 10.294 / p99 11.964 / p99.9 27.972 / max 131.680 / mean 7.340 / stddev 2.826
**load-2/4-c32 end-to-end latency (ms)**: p50 444.672 / p90 1175.504 / p95 1400.483 / p99 1784.485 / p99.9 2150.725 / max 2327.975 / mean 553.854 / stddev 421.138

| load-3/4-c64 | 64 | 25000 | 25000 | 0 | 0 | 58.55 | 0.0000% | 426.96 |

**load-3/4-c64 start API latency (ms)**: p50 7.219 / p90 9.695 / p95 10.482 / p99 12.620 / p99.9 118.212 / max 488.366 / mean 7.801 / stddev 9.683
**load-3/4-c64 end-to-end latency (ms)**: p50 844.006 / p90 2407.973 / p95 2907.377 / p99 3827.154 / p99.9 4705.684 / max 5380.170 / mean 1091.653 / stddev 908.121

| load-4/4-c128 | 128 | 25000 | 25000 | 0 | 0 | 58.00 | 0.0000% | 431.05 |

**load-4/4-c128 start API latency (ms)**: p50 7.590 / p90 10.013 / p95 10.815 / p99 13.068 / p99.9 143.893 / max 421.235 / mean 8.291 / stddev 8.294
**load-4/4-c128 end-to-end latency (ms)**: p50 1959.252 / p90 4242.024 / p95 5050.162 / p99 7005.423 / p99.9 8878.343 / max 9963.815 / mean 2203.955 / stddev 1531.882

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 53.09 | 0.0000% | 1.88 |

**post-stress-smoke start API latency (ms)**: p50 7.763 / p90 16.856 / p95 19.352 / p99 23.090 / p99.9 23.436 / max 23.436 / mean 9.158 / stddev 4.469
**post-stress-smoke end-to-end latency (ms)**: p50 236.884 / p90 526.631 / p95 572.902 / p99 655.799 / p99.9 695.045 / max 695.045 / mean 285.807 / stddev 167.727

## Metric definitions and limitations

- Each run's `input` varies 0–9, so the while-loop length and both branches are exercised.
- The report keeps status metrics only; it never copies `SystemStatus.config`, so credentials in configuration cannot leak into a report.
- start API latency: from the `proc:start` call until a durable run handle is returned.
- end-to-end latency: from submission until `RunHandle::wait` observes a terminal outcome, including queueing and execution.
- workflows/s: completed runs divided by the phase's wall-clock time; errored and terminated runs do not count.
- Percentiles use nearest-rank; stddev is the sample standard deviation.
- CPU/RSS/storage/process counts/background loop health come from the engine's `sys:status`; each CPU sample waits for sysinfo's minimum CPU delta window, so the actual sampling interval is at least `sample-ms` and the sampler itself adds some overhead. Peaks are sample points, not continuous peak values. Collection failures are counted and fail the PASS gate.
- This is a closed-loop load: each worker submits its next run only after the previous one finished; it is not a fixed-arrival-rate open-loop model.
- The backend is selected by `--store`: `sled` (default, `./data`), `memory`, `sqlite` (file in `--store-url` or `<output>/stress.sqlite`), or `postgres` (URL in `--store-url` or `DATABASE_URL`; never copied into this report). `sled` acknowledges before fsync and flushes in the background every 50ms, so a crash can lose that window (and the measured throughput excludes the final shutdown flush); SQLite/Postgres make each commit durable before it returns; `memory` isolates engine cost from storage cost.
- PASS means the measured target was submitted and every measured/soak/post-stress phase stayed under the error-rate gate, with no wait timeouts, a complete post-stress smoke run, healthy background loops and successful metric collection. A few failures under the gate are still reported. Use `--max-error-rate-pct 0` for strict zero-failure gating. PASS is not a performance SLO claim for every workload.

## Output files

- `report.json`: full structured report.
- `metrics.csv`: system resource and background health time series.
- `report.md`: this report.
- `trace.log`: tracing log for this run (controlled by `--log-level` / `ACTS_LOG`, default `info`; use `acts=debug,acts_example_stress=debug` for bottleneck analysis and `acts=trace` to see every operation's worker lane. Each `scheduler lane executing operation` line is one plan transition; count them per pid to measure transitions/workflow).
