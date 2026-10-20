# stroke_replay_matrix: `pencil`

Adapter: Intel(R) Graphics (RPL-U) (Vulkan, Intel open-source Mesa driver Mesa 26.2.3-arch1.1).

Brush: `Pencil` topology `pencil` (terminal: `paint`, stabilize=`1`, buildup=`brush`, gpu_sync=`false`). Recording: 204 events spanning 3536 ms recorded at 4000×2000. Replay pacing: real-time. `behind_by_ms = wall_total - stroke_duration`: positive means the engine fell behind the recorded cadence. `max_event_behind_ms` is the worst single-event lateness (`cpu_ms - inter_event_gap_ms`, clamped at zero, max across events).

Markdown carries the slim view; the sibling TSV has p95/max for every column. `submit` is host wall-clock around `queue.submit()`: high values indicate back-pressure. `flushes/ev`, `dispatches/ev`, `dabs/ev`, `bbox/ev` are per-event averages of the workload the engine fed the GPU: flushes are `flush_dabs` calls, dispatches are draws or compute dispatches (one per flush for an instanced terminal, one per dab for a serialized one, two per dab for a brush that samples the live stroke). The 6-slot GPU-timestamp columns (`gpu_shader` / `gpu_sync_in` / `gpu_sync_out`) that the older matrices carried are gone; they instrumented the compute-path buffer round-trip, which the `paint` terminal no longer pays.

| canvas | radius_px | events | wall (ms) | behind (ms) | worst-frame (ms) | cpu p50 (µs) | submit p50 (µs) | flushes/ev | dispatches/ev | dabs/ev | bbox px²/ev | fallbacks |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1920×1080 | 1 | 204 | 3543 | +7 | 19.4 | 7467 | 2310 | 9.54 | 490.9 | 490.9 | 11364 | 0 |
| 1920×1080 | 10 | 204 | 3539 | +3 | 26.2 | 7747 | 2332 | 9.54 | 491.0 | 491.0 | 16133 | 0 |
| 1920×1080 | 100 | 204 | 3544 | +8 | 20.8 | 7263 | 2566 | 9.54 | 301.9 | 301.9 | 123495 | 0 |
| 2560×1440 | 2000 | 204 | 9208 | +5672 | 171.7 | 10530 | 5373 | 9.35 | 20.5 | 20.5 | 21758686 | 0 |
