# Acts million-run stress report

- Generated at (Unix seconds): 1793664341
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 774.09s
- Measured throughput: 129.18 workflows/s
- Peak RSS: 0.34 GiB
- Min system available memory: 14.31 GiB
- Process CPU: peak 380.7% / sample mean 323.0%
- System CPU: peak 92.9% / sample mean 81.6%
- Max storage used: 0.15 GiB
- Peak process rows: 2315 (active 129)
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
- Store shards: 1
- Cache cap: 320
- Scheduler queue cap: 4096
- Store writer queue cap: 4096
- Metric sample interval: 1000ms (each status sample also waits for sysinfo's CPU delta window)
- Per-workflow wait timeout: 120s
- Engine version: 0.1.0
- Store backend: memory
- Error-rate gate: 0.000%

Backend selected by --store; connection URLs are not copied into this report; closed-loop load; sampled peaks may understate instantaneous peaks. Run a release build on the target machine.

## Per-phase results

| Phase | Concurrency | Attempted | Completed | Errored | Terminated | Throughput/s | Error rate | Elapsed/s |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| warmup | 16 | 100 | 100 | 0 | 0 | 208.62 | 0.0000% | 0.48 |

**warmup start API latency (ms)**: p50 0.670 / p90 0.937 / p95 0.985 / p99 1.239 / p99.9 1.398 / max 1.398 / mean 0.713 / stddev 0.148
**warmup end-to-end latency (ms)**: p50 66.525 / p90 124.189 / p95 133.476 / p99 156.955 / p99.9 157.645 / max 157.645 / mean 70.782 / stddev 35.684

| load-1/4-c16 | 16 | 25000 | 25000 | 0 | 0 | 129.88 | 0.0000% | 192.48 |

**load-1/4-c16 start API latency (ms)**: p50 0.890 / p90 1.650 / p95 2.088 / p99 2.875 / p99.9 39.789 / max 119.915 / mean 1.160 / stddev 2.370
**load-1/4-c16 end-to-end latency (ms)**: p50 99.108 / p90 250.431 / p95 308.782 / p99 458.659 / p99.9 620.169 / max 714.813 / mean 123.140 / stddev 96.507

| load-2/4-c32 | 32 | 25000 | 25000 | 0 | 0 | 128.57 | 0.0000% | 194.45 |

**load-2/4-c32 start API latency (ms)**: p50 0.919 / p90 1.648 / p95 2.026 / p99 2.690 / p99.9 26.827 / max 86.422 / mean 1.125 / stddev 1.646
**load-2/4-c32 end-to-end latency (ms)**: p50 201.471 / p90 516.661 / p95 639.346 / p99 931.580 / p99.9 1245.434 / max 1458.960 / mean 248.710 / stddev 200.964

| load-3/4-c64 | 64 | 25000 | 25000 | 0 | 0 | 129.17 | 0.0000% | 193.54 |

**load-3/4-c64 start API latency (ms)**: p50 0.923 / p90 1.649 / p95 2.023 / p99 2.572 / p99.9 26.276 / max 93.553 / mean 1.117 / stddev 1.563
**load-3/4-c64 end-to-end latency (ms)**: p50 362.022 / p90 1108.509 / p95 1390.609 / p99 2063.432 / p99.9 2657.592 / max 2975.882 / mean 494.989 / stddev 449.387

| load-4/4-c128 | 128 | 25000 | 25000 | 0 | 0 | 129.12 | 0.0000% | 193.62 |

**load-4/4-c128 start API latency (ms)**: p50 0.927 / p90 1.691 / p95 2.051 / p99 2.565 / p99.9 6.404 / max 79.834 / mean 1.092 / stddev 1.112
**load-4/4-c128 end-to-end latency (ms)**: p50 791.478 / p90 2057.589 / p95 2527.990 / p99 3742.654 / p99.9 5015.874 / max 5549.553 / mean 989.571 / stddev 804.601

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 144.43 | 0.0000% | 0.69 |

**post-stress-smoke start API latency (ms)**: p50 0.851 / p90 1.169 / p95 1.287 / p99 3.971 / p99.9 24.312 / max 24.312 / mean 1.164 / stddev 2.354
**post-stress-smoke end-to-end latency (ms)**: p50 77.367 / p90 207.941 / p95 222.108 / p99 242.599 / p99.9 268.343 / max 268.343 / mean 98.934 / stddev 62.941

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
