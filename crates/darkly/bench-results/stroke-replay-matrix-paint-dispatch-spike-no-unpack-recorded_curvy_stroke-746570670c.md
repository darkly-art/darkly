# stroke_replay_matrix: `paint-dispatch-spike`

Adapter: Intel(R) Graphics (RPL-U) (Vulkan, Intel open-source Mesa driver Mesa 26.2.3-arch1.1).

Brush: `Ink Pen (dispatch spike)` topology `paint-dispatch-spike` (terminal: `paint_dispatch_spike`, stabilize=`1`). Recording: 204 events spanning 3536 ms recorded at 4000×2000. Replay pacing: real-time. `behind_by_ms = wall_total - stroke_duration`: positive means the engine fell behind the recorded cadence. `max_event_behind_ms` is the worst single-event lateness (`cpu_ms - inter_event_gap_ms`, clamped at zero, max across events).

Markdown carries the slim view; the sibling TSV has p95/max for every column. `submit` is host wall-clock around `queue.submit()`: high values indicate back-pressure. `flushes/ev`, `dispatches/ev`, `dabs/ev`, `bbox/ev` are per-event averages of the workload the engine fed the GPU: flushes are `flush_dabs` calls, dispatches are draws or compute dispatches into the scratch (one per flush for an instanced terminal, one per dab for a serialized one). The 6-slot GPU-timestamp columns (`gpu_shader` / `gpu_sync_in` / `gpu_sync_out`) that the older matrices carried are gone; they instrumented the compute-path buffer round-trip, which the `paint` terminal no longer pays.

| canvas | radius_px | events | wall (ms) | behind (ms) | worst-frame (ms) | cpu p50 (µs) | submit p50 (µs) | flushes/ev | dispatches/ev | dabs/ev | bbox px²/ev |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1280×720 | 1 | 204 | 3550 | +14 | 16.3 | 6451 | 2535 | 9.30 | 321.4 | 321.4 | 5049 |
| 1280×720 | 10 | 204 | 3551 | +15 | 16.4 | 6582 | 2567 | 9.30 | 321.5 | 321.5 | 9660 |
| 1280×720 | 100 | 204 | 3553 | +17 | 16.3 | 4246 | 2165 | 9.10 | 32.3 | 32.3 | 124903 |
| 1280×720 | 250 | 204 | 3552 | +16 | 24.9 | 4059 | 2130 | 8.17 | 13.0 | 13.0 | 551799 |
| 1280×720 | 500 | 204 | 3554 | +18 | 19.1 | 3760 | 2003 | 5.87 | 6.5 | 6.5 | 1452269 |
| 1280×720 | 1000 | 204 | 3555 | +19 | 16.4 | 3476 | 1930 | 3.23 | 3.3 | 3.3 | 2467817 |
| 1280×720 | 2000 | 204 | 3554 | +18 | 22.0 | 3331 | 1858 | 1.65 | 1.7 | 1.7 | 2221146 |
| 1920×1080 | 1 | 204 | 3550 | +14 | 23.2 | 7124 | 2680 | 9.54 | 490.9 | 490.9 | 11052 |
| 1920×1080 | 10 | 204 | 3549 | +13 | 19.9 | 7084 | 2697 | 9.54 | 491.0 | 491.0 | 17614 |
| 1920×1080 | 100 | 204 | 3554 | +18 | 23.3 | 4370 | 2124 | 9.47 | 49.2 | 49.2 | 154293 |
| 1920×1080 | 250 | 204 | 3555 | +19 | 24.1 | 4393 | 2395 | 9.04 | 19.7 | 19.7 | 657283 |
| 1920×1080 | 500 | 204 | 3554 | +18 | 23.5 | 4377 | 2403 | 7.53 | 9.9 | 9.9 | 1956006 |
| 1920×1080 | 1000 | 204 | 3557 | +21 | 34.4 | 4606 | 2505 | 4.74 | 5.0 | 5.0 | 4533434 |
| 1920×1080 | 2000 | 204 | 3557 | +21 | 37.6 | 4377 | 2401 | 2.50 | 2.5 | 2.5 | 6227740 |
| 2560×1440 | 1 | 204 | 3552 | +16 | 20.4 | 7155 | 2624 | 9.80 | 666.8 | 666.8 | 19337 |
| 2560×1440 | 10 | 204 | 3552 | +16 | 21.4 | 7275 | 2671 | 9.80 | 666.9 | 666.9 | 27891 |
| 2560×1440 | 100 | 204 | 3553 | +17 | 22.1 | 5146 | 2456 | 9.74 | 66.8 | 66.8 | 185965 |
| 2560×1440 | 250 | 204 | 3558 | +22 | 33.8 | 5135 | 2688 | 9.52 | 26.7 | 26.7 | 744555 |
| 2560×1440 | 500 | 204 | 3556 | +20 | 25.8 | 5141 | 2751 | 8.55 | 13.3 | 13.3 | 2294980 |
| 2560×1440 | 1000 | 204 | 3556 | +20 | 24.7 | 4924 | 2715 | 6.05 | 6.7 | 6.7 | 5964365 |
| 2560×1440 | 2000 | 204 | 3576 | +40 | 17.2 | 6097 | 3153 | 3.28 | 3.3 | 3.3 | 9826570 |
| 3840×2160 | 1 | 204 | 3553 | +17 | 24.3 | 7745 | 2730 | 10.06 | 1021.4 | 1021.4 | 43332 |
| 3840×2160 | 10 | 204 | 3552 | +16 | 24.7 | 7951 | 2833 | 10.06 | 1021.5 | 1021.5 | 55986 |
| 3840×2160 | 100 | 204 | 3552 | +16 | 25.1 | 6468 | 2806 | 10.03 | 102.2 | 102.2 | 254862 |
| 3840×2160 | 250 | 204 | 3554 | +18 | 51.2 | 6382 | 3044 | 9.94 | 40.9 | 40.9 | 897561 |
| 3840×2160 | 500 | 204 | 3556 | +20 | 54.3 | 6683 | 3424 | 9.52 | 20.5 | 20.5 | 2747457 |
| 3840×2160 | 1000 | 204 | 3981 | +445 | 36.9 | 20986 | 3029 | 7.89 | 10.3 | 10.3 | 8139388 |
| 3840×2160 | 2000 | 204 | 5901 | +2365 | 84.5 | 31227 | 2949 | 4.94 | 5.1 | 5.1 | 18152638 |
