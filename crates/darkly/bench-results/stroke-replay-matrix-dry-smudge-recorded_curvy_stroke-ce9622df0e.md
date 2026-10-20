# stroke_replay_matrix: `dry-smudge`

Adapter: Intel(R) Graphics (RPL-U) (Vulkan, Intel open-source Mesa driver Mesa 26.2.3-arch1.1).

Brush: `Dry Smudge` topology `dry-smudge` (terminal: `paint`, stabilize=`1`, buildup=`brush`, gpu_sync=`false`). Recording: 204 events spanning 3536 ms recorded at 4000×2000. Replay pacing: real-time. `behind_by_ms = wall_total - stroke_duration`: positive means the engine fell behind the recorded cadence. `max_event_behind_ms` is the worst single-event lateness (`cpu_ms - inter_event_gap_ms`, clamped at zero, max across events).

Markdown carries the slim view; the sibling TSV has p95/max for every column. `submit` is host wall-clock around `queue.submit()`: high values indicate back-pressure. `flushes/ev`, `dispatches/ev`, `dabs/ev`, `bbox/ev` are per-event averages of the workload the engine fed the GPU: flushes are `flush_dabs` calls, dispatches are draws or compute dispatches (one per flush for an instanced terminal, one per dab for a serialized one, two per dab for a brush that samples the live stroke). The 6-slot GPU-timestamp columns (`gpu_shader` / `gpu_sync_in` / `gpu_sync_out`) that the older matrices carried are gone; they instrumented the compute-path buffer round-trip, which the `paint` terminal no longer pays.

| canvas | radius_px | events | wall (ms) | behind (ms) | worst-frame (ms) | cpu p50 (µs) | submit p50 (µs) | flushes/ev | dispatches/ev | dabs/ev | bbox px²/ev | fallbacks |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1920×1080 | 1 | 204 | 3544 | +8 | 22.0 | 7455 | 3800 | 9.54 | 981.9 | 490.9 | 11267 | 0 |
| 1920×1080 | 10 | 204 | 3542 | +6 | 29.0 | 7322 | 3818 | 9.54 | 982.1 | 491.0 | 20567 | 0 |
| 1920×1080 | 100 | 204 | 3542 | +6 | 21.0 | 6039 | 3425 | 9.54 | 242.8 | 121.4 | 262588 | 0 |
| 2560×1440 | 2000 | 204 | 5859 | +2323 | 95.3 | 6879 | 4549 | 6.96 | 16.5 | 8.3 | 27961702 | 0 |
