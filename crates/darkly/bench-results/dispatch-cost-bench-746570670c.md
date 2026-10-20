# dispatch_cost_bench at `746570670c`

Adapter: Intel(R) Graphics (RPL-U) (Vulkan, driver Intel open-source Mesa driver Mesa 26.2.3-arch1.1). Target 3840x2160. 20 iterations per cell, p50 reported. Pass timestamps: yes.

| radius_px | N | shape | wall p50 (ms) | wall min (ms) | wall per dab, p50 (us) | gpu p50 (ms) | gpu per dab, p50 (us) | parity max diff (LSB) |
|---:|---:|---|---:|---:|---:|---:|---:|---:|
| 1.5 | 300 | (a) dispatch, read-write | 4.51 | 4.30 | 15.0 | 2.79 | 9.3 | 1 |
| 1.5 | 300 | (b) dispatch, read-only | 4.91 | 4.37 | 16.4 | 2.70 | 9.0 | - |
| 1.5 | 300 | (c) render pass per dab | 28.15 | 27.55 | 93.8 | 11.53 | 38.4 | - |
| 1.5 | 900 | (a) dispatch, read-write | 5.23 | 4.92 | 5.8 | 3.68 | 4.1 | 1 |
| 1.5 | 900 | (b) dispatch, read-only | 5.63 | 5.27 | 6.3 | 3.58 | 4.0 | - |
| 1.5 | 900 | (c) render pass per dab | 42.45 | 39.48 | 47.2 | 15.08 | 16.8 | - |
| 1.5 | 2000 | (a) dispatch, read-write | 7.29 | 7.03 | 3.6 | 4.93 | 2.5 | 1 |
| 1.5 | 2000 | (b) dispatch, read-only | 7.55 | 7.14 | 3.8 | 4.77 | 2.4 | - |
| 1.5 | 2000 | (c) render pass per dab | 72.61 | 68.18 | 36.3 | 19.32 | 9.7 | - |
| 10 | 300 | (a) dispatch, read-write | 2.20 | 1.99 | 7.3 | 0.86 | 2.9 | 1 |
| 10 | 300 | (b) dispatch, read-only | 2.35 | 2.09 | 7.8 | 0.83 | 2.8 | - |
| 10 | 300 | (c) render pass per dab | 13.97 | 13.11 | 46.6 | 3.45 | 11.5 | - |
| 10 | 900 | (a) dispatch, read-write | 5.37 | 4.42 | 6.0 | 3.40 | 3.8 | 2 |
| 10 | 900 | (b) dispatch, read-only | 5.83 | 4.59 | 6.5 | 3.26 | 3.6 | - |
| 10 | 900 | (c) render pass per dab | 42.54 | 35.85 | 47.3 | 13.57 | 15.1 | - |
| 10 | 2000 | (a) dispatch, read-write | 9.61 | 7.60 | 4.8 | 6.59 | 3.3 | 2 |
| 10 | 2000 | (b) dispatch, read-only | 9.87 | 7.35 | 4.9 | 6.17 | 3.1 | - |
| 10 | 2000 | (c) render pass per dab | 83.48 | 69.37 | 41.7 | 25.66 | 12.8 | - |
| 1000 | 5 | (a) dispatch, read-write | 7.23 | 6.81 | 1446.0 | 5.08 | 1016.6 | 3 |
| 1000 | 5 | (b) dispatch, read-only | 5.80 | 5.40 | 1160.9 | 3.74 | 748.7 | - |
| 1000 | 5 | (c) render pass per dab | 4.78 | 4.08 | 956.8 | 2.40 | 479.1 | - |
| 1000 | 10 | (a) dispatch, read-write | 12.50 | 12.27 | 1250.4 | 10.23 | 1023.5 | 4 |
| 1000 | 10 | (b) dispatch, read-only | 9.64 | 9.46 | 963.7 | 7.55 | 754.9 | - |
| 1000 | 10 | (c) render pass per dab | 7.54 | 7.31 | 754.5 | 4.80 | 480.3 | - |
