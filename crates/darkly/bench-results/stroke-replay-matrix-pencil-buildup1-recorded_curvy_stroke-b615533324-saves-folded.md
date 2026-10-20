# stroke_replay_matrix: `pencil`

Adapter: Intel(R) Graphics (RPL-U) (Vulkan, Intel open-source Mesa driver Mesa 26.2.3-arch1.1).

Brush: `Pencil` topology `pencil` (terminal: `paint`, stabilize=`0.6`, buildup=`1`, gpu_sync=`true`). Recording: 204 events spanning 3536 ms recorded at 4000×2000. Replay pacing: real-time. `behind_by_ms = wall_total - stroke_duration`: positive means the engine fell behind the recorded cadence. `max_event_behind_ms` is the worst single-event lateness (`cpu_ms - inter_event_gap_ms`, clamped at zero, max across events).

Markdown carries the slim view; the sibling TSV has p95/max for every column. `submit` is host wall-clock around `queue.submit()`: high values indicate back-pressure. `flushes/ev`, `dispatches/ev`, `dabs/ev`, `bbox/ev` are per-event averages of the workload the engine fed the GPU: flushes are `flush_dabs` calls, dispatches are draws or compute dispatches into the scratch (one per flush for an instanced terminal, one per dab for a serialized one). The 6-slot GPU-timestamp columns (`gpu_shader` / `gpu_sync_in` / `gpu_sync_out`) that the older matrices carried are gone; they instrumented the compute-path buffer round-trip, which the `paint` terminal no longer pays.

| canvas | radius_px | events | wall (ms) | behind (ms) | worst-frame (ms) | cpu p50 (µs) | submit p50 (µs) | flushes/ev | dispatches/ev | dabs/ev | bbox px²/ev | fallbacks |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1920×1080 | 250 | 204 | 3544 | +8 | 16.8 | 9056 | 1701 | 5.85 | 116.5 | 116.5 | 374850 | 0 |
| 1920×1080 | 500 | 204 | 3544 | +8 | 15.6 | 10614 | 1486 | 5.83 | 58.3 | 58.3 | 1196443 | 0 |
