# stroke_replay_matrix: `paint`

Adapter: Intel(R) Graphics (RPL-U) (Vulkan, Intel open-source Mesa driver Mesa 26.2.3-arch1.1).

Brush: `Ink Pen` topology `paint` (terminal: `paint`, stabilize=`1`). Recording: 204 events spanning 3536 ms recorded at 4000×2000. Replay pacing: real-time. `behind_by_ms = wall_total - stroke_duration`: positive means the engine fell behind the recorded cadence. `max_event_behind_ms` is the worst single-event lateness (`cpu_ms - inter_event_gap_ms`, clamped at zero, max across events).

Markdown carries the slim view; the sibling TSV has p95/max for every column. `submit` is host wall-clock around `queue.submit()`: high values indicate back-pressure. `flushes/ev`, `dispatches/ev`, `dabs/ev`, `bbox/ev` are per-event averages of the workload the engine fed the GPU: flushes are `flush_dabs` calls, dispatches are draws or compute dispatches into the scratch (one per flush for an instanced terminal, one per dab for a serialized one). The 6-slot GPU-timestamp columns (`gpu_shader` / `gpu_sync_in` / `gpu_sync_out`) that the older matrices carried are gone; they instrumented the compute-path buffer round-trip, which the `paint` terminal no longer pays.

| canvas | radius_px | events | wall (ms) | behind (ms) | worst-frame (ms) | cpu p50 (µs) | submit p50 (µs) | flushes/ev | dispatches/ev | dabs/ev | bbox px²/ev |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1280×720 | 1 | 204 | 3540 | +4 | 3.8 | 6835 | 2744 | 9.30 | 321.4 | 321.4 | 5049 |
| 1280×720 | 10 | 204 | 3542 | +6 | 3.1 | 6915 | 2752 | 9.30 | 321.5 | 321.5 | 9660 |
| 1280×720 | 100 | 204 | 3542 | +6 | 3.0 | 4657 | 2350 | 9.10 | 32.3 | 32.3 | 124903 |
| 1280×720 | 250 | 204 | 3546 | +10 | 24.8 | 4434 | 2351 | 8.17 | 13.0 | 13.0 | 551799 |
| 1280×720 | 500 | 204 | 3546 | +10 | 35.9 | 3979 | 2073 | 5.87 | 6.5 | 6.5 | 1452269 |
| 1280×720 | 1000 | 204 | 3545 | +9 | 24.1 | 3856 | 2043 | 3.23 | 3.3 | 3.3 | 2467817 |
| 1280×720 | 2000 | 204 | 3544 | +8 | 23.7 | 3647 | 1915 | 1.65 | 1.7 | 1.7 | 2221146 |
| 1920×1080 | 1 | 204 | 3544 | +8 | 19.9 | 7238 | 2865 | 9.54 | 490.9 | 490.9 | 11052 |
| 1920×1080 | 10 | 204 | 3542 | +6 | 19.5 | 7426 | 2899 | 9.54 | 491.0 | 491.0 | 17614 |
| 1920×1080 | 100 | 204 | 3542 | +6 | 30.4 | 5262 | 2580 | 9.47 | 49.2 | 49.2 | 154293 |
| 1920×1080 | 250 | 204 | 3545 | +9 | 26.2 | 4716 | 2436 | 9.04 | 19.7 | 19.7 | 657283 |
| 1920×1080 | 500 | 204 | 3546 | +10 | 21.3 | 4543 | 2402 | 7.53 | 9.9 | 9.9 | 1956006 |
| 1920×1080 | 1000 | 204 | 3545 | +9 | 18.0 | 4399 | 2428 | 4.74 | 5.0 | 5.0 | 4533434 |
| 1920×1080 | 2000 | 204 | 3546 | +10 | 32.1 | 4393 | 2420 | 2.50 | 2.5 | 2.5 | 6227740 |
| 2560×1440 | 1 | 204 | 3544 | +8 | 20.1 | 7635 | 2937 | 9.80 | 666.8 | 666.8 | 19337 |
| 2560×1440 | 10 | 204 | 3543 | +7 | 19.0 | 7315 | 2916 | 9.80 | 666.9 | 666.9 | 27891 |
| 2560×1440 | 100 | 204 | 3543 | +7 | 26.6 | 5630 | 2782 | 9.74 | 66.8 | 66.8 | 185965 |
| 2560×1440 | 250 | 204 | 3544 | +8 | 18.4 | 5241 | 2694 | 9.52 | 26.7 | 26.7 | 744555 |
| 2560×1440 | 500 | 204 | 3546 | +10 | 24.1 | 5057 | 2654 | 8.55 | 13.3 | 13.3 | 2294980 |
| 2560×1440 | 1000 | 204 | 3548 | +12 | 23.8 | 4985 | 2726 | 6.05 | 6.7 | 6.7 | 5964365 |
| 2560×1440 | 2000 | 204 | 3547 | +11 | 3.4 | 5194 | 2812 | 3.28 | 3.3 | 3.3 | 9826570 |
| 3840×2160 | 1 | 204 | 3543 | +7 | 23.4 | 7954 | 2827 | 10.06 | 1021.4 | 1021.4 | 43332 |
| 3840×2160 | 10 | 204 | 3544 | +8 | 22.7 | 8185 | 3009 | 10.06 | 1021.5 | 1021.5 | 55986 |
| 3840×2160 | 100 | 204 | 3545 | +9 | 26.5 | 6294 | 2884 | 10.03 | 102.2 | 102.2 | 254862 |
| 3840×2160 | 250 | 204 | 3546 | +10 | 22.7 | 6006 | 2975 | 9.94 | 40.9 | 40.9 | 897561 |
| 3840×2160 | 500 | 204 | 3545 | +9 | 25.3 | 6154 | 3299 | 9.52 | 20.5 | 20.5 | 2747457 |
| 3840×2160 | 1000 | 204 | 3547 | +11 | 35.1 | 6793 | 3620 | 7.89 | 10.3 | 10.3 | 8139388 |
| 3840×2160 | 2000 | 204 | 4891 | +1355 | 77.3 | 25863 | 3249 | 4.94 | 5.1 | 5.1 | 18152638 |
