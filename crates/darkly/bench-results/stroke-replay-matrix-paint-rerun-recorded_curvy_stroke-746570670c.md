# stroke_replay_matrix: `paint`

Adapter: Intel(R) Graphics (RPL-U) (Vulkan, Intel open-source Mesa driver Mesa 26.2.3-arch1.1).

Brush: `Ink Pen` topology `paint` (terminal: `paint`, stabilize=`1`). Recording: 204 events spanning 3536 ms recorded at 4000×2000. Replay pacing: real-time. `behind_by_ms = wall_total - stroke_duration`: positive means the engine fell behind the recorded cadence. `max_event_behind_ms` is the worst single-event lateness (`cpu_ms - inter_event_gap_ms`, clamped at zero, max across events).

Markdown carries the slim view; the sibling TSV has p95/max for every column. `submit` is host wall-clock around `queue.submit()`: high values indicate back-pressure. `flushes/ev`, `dispatches/ev`, `dabs/ev`, `bbox/ev` are per-event averages of the workload the engine fed the GPU: flushes are `flush_dabs` calls, dispatches are draws or compute dispatches into the scratch (one per flush for an instanced terminal, one per dab for a serialized one). The 6-slot GPU-timestamp columns (`gpu_shader` / `gpu_sync_in` / `gpu_sync_out`) that the older matrices carried are gone; they instrumented the compute-path buffer round-trip, which the `paint` terminal no longer pays.

| canvas | radius_px | events | wall (ms) | behind (ms) | worst-frame (ms) | cpu p50 (µs) | submit p50 (µs) | flushes/ev | dispatches/ev | dabs/ev | bbox px²/ev |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1280×720 | 1 | 204 | 3541 | +5 | 3.4 | 5829 | 1808 | 9.30 | 9.3 | 321.4 | 5049 |
| 1280×720 | 10 | 204 | 3541 | +5 | 2.7 | 5653 | 1794 | 9.30 | 9.3 | 321.5 | 9660 |
| 1280×720 | 100 | 204 | 3542 | +6 | 2.7 | 3684 | 1903 | 9.10 | 9.1 | 32.3 | 124903 |
| 1280×720 | 250 | 204 | 3544 | +8 | 24.3 | 3653 | 2051 | 8.17 | 8.2 | 13.0 | 551799 |
| 1280×720 | 500 | 204 | 3544 | +8 | 23.0 | 3403 | 1934 | 5.87 | 5.9 | 6.5 | 1452269 |
| 1280×720 | 1000 | 204 | 3544 | +8 | 22.6 | 3388 | 1886 | 3.23 | 3.2 | 3.3 | 2467817 |
| 1280×720 | 2000 | 204 | 3544 | +8 | 24.4 | 3073 | 1728 | 1.65 | 1.7 | 1.7 | 2221146 |
| 1920×1080 | 1 | 204 | 3541 | +5 | 19.9 | 6616 | 1799 | 9.54 | 9.5 | 490.9 | 11052 |
| 1920×1080 | 10 | 204 | 3541 | +5 | 20.5 | 6550 | 1729 | 9.54 | 9.5 | 491.0 | 17614 |
| 1920×1080 | 100 | 204 | 3541 | +5 | 23.7 | 4026 | 2018 | 9.47 | 9.5 | 49.2 | 154293 |
| 1920×1080 | 250 | 204 | 3545 | +9 | 34.2 | 4050 | 2201 | 9.04 | 9.0 | 19.7 | 657283 |
| 1920×1080 | 500 | 204 | 3545 | +9 | 18.2 | 3771 | 2090 | 7.53 | 7.5 | 9.9 | 1956006 |
| 1920×1080 | 1000 | 204 | 3543 | +7 | 19.9 | 3842 | 2173 | 4.74 | 4.7 | 5.0 | 4533434 |
| 1920×1080 | 2000 | 204 | 3545 | +9 | 22.1 | 3563 | 1976 | 2.50 | 2.5 | 2.5 | 6227740 |
| 2560×1440 | 1 | 204 | 3540 | +4 | 19.4 | 6972 | 1664 | 9.80 | 9.8 | 666.8 | 19337 |
| 2560×1440 | 10 | 204 | 3541 | +5 | 19.7 | 6885 | 1646 | 9.80 | 9.8 | 666.9 | 27891 |
| 2560×1440 | 100 | 204 | 3541 | +5 | 23.6 | 4419 | 2222 | 9.74 | 9.7 | 66.8 | 185965 |
| 2560×1440 | 250 | 204 | 3544 | +8 | 17.5 | 3858 | 2145 | 9.52 | 9.5 | 26.7 | 744555 |
| 2560×1440 | 500 | 204 | 3543 | +7 | 24.7 | 4013 | 2261 | 8.55 | 8.6 | 13.3 | 2294980 |
| 2560×1440 | 1000 | 204 | 3544 | +8 | 25.1 | 4043 | 2265 | 6.05 | 6.0 | 6.7 | 5964365 |
| 2560×1440 | 2000 | 204 | 3544 | +8 | 4.5 | 3897 | 2216 | 3.28 | 3.3 | 3.3 | 9826570 |
| 3840×2160 | 1 | 204 | 3542 | +6 | 22.5 | 7071 | 1298 | 10.06 | 10.1 | 1021.4 | 43332 |
| 3840×2160 | 10 | 204 | 3542 | +6 | 22.5 | 7032 | 1339 | 10.06 | 10.1 | 1021.5 | 55986 |
| 3840×2160 | 100 | 204 | 3541 | +5 | 25.0 | 4355 | 1952 | 10.03 | 10.0 | 102.2 | 254862 |
| 3840×2160 | 250 | 204 | 3545 | +9 | 18.0 | 4500 | 2217 | 9.94 | 9.9 | 40.9 | 897561 |
| 3840×2160 | 500 | 204 | 3545 | +9 | 25.2 | 4659 | 2524 | 9.52 | 9.5 | 20.5 | 2747457 |
| 3840×2160 | 1000 | 204 | 3544 | +8 | 25.6 | 5216 | 2928 | 7.89 | 7.9 | 10.3 | 8139388 |
| 3840×2160 | 2000 | 204 | 3549 | +13 | 34.1 | 5869 | 2999 | 4.94 | 4.9 | 5.1 | 18152638 |
