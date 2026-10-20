# dispatch_cost_bench at `929928f9ab`

Adapter: Intel(R) Graphics (RPL-U) (Vulkan, driver Intel open-source Mesa driver Mesa 26.2.3-arch1.1). Target 3840x2160. 20 iterations per cell, p50 reported. Pass timestamps: yes.

| radius_px | N | shape | wall p50 (ms) | wall min (ms) | wall per dab, p50 (us) | gpu p50 (ms) | gpu per dab, p50 (us) | parity max diff (LSB) |
|---:|---:|---|---:|---:|---:|---:|---:|---:|
| 1.5 | 300 | (a) dispatch, read-write | 3.24 | 3.03 | 10.8 | 1.89 | 6.3 | 1 |
| 1.5 | 300 | (a16) dispatch, read-write, 16x16 | 4.01 | 3.55 | 13.4 | 2.23 | 7.4 | - |
| 1.5 | 300 | (a-rows) dispatch, read-write, 4 rows per thread | 4.76 | 3.54 | 15.9 | 2.73 | 9.1 | - |
| 1.5 | 300 | (b) dispatch, read-only | 3.95 | 3.30 | 13.2 | 1.83 | 6.1 | - |
| 1.5 | 300 | (c) render pass per dab | 24.19 | 20.06 | 80.6 | 7.89 | 26.3 | - |
| 1.5 | 900 | (a) dispatch, read-write | 5.30 | 4.85 | 5.9 | 3.48 | 3.9 | 1 |
| 1.5 | 900 | (a16) dispatch, read-write, 16x16 | 7.65 | 6.09 | 8.5 | 4.16 | 4.6 | - |
| 1.5 | 900 | (a-rows) dispatch, read-write, 4 rows per thread | 8.51 | 7.17 | 9.5 | 5.01 | 5.6 | - |
| 1.5 | 900 | (b) dispatch, read-only | 7.16 | 6.21 | 8.0 | 3.46 | 3.8 | - |
| 1.5 | 900 | (c) render pass per dab | 48.39 | 42.57 | 53.8 | 14.41 | 16.0 | - |
| 1.5 | 2000 | (a) dispatch, read-write | 7.70 | 6.97 | 3.9 | 5.26 | 2.6 | 1 |
| 1.5 | 2000 | (a16) dispatch, read-write, 16x16 | 9.21 | 8.04 | 4.6 | 6.39 | 3.2 | - |
| 1.5 | 2000 | (a-rows) dispatch, read-write, 4 rows per thread | 11.36 | 10.00 | 5.7 | 7.44 | 3.7 | - |
| 1.5 | 2000 | (b) dispatch, read-only | 11.04 | 9.00 | 5.5 | 5.27 | 2.6 | - |
| 1.5 | 2000 | (c) render pass per dab | 77.93 | 72.47 | 39.0 | 21.13 | 10.6 | - |
| 10 | 300 | (a) dispatch, read-write | 2.33 | 2.08 | 7.8 | 0.87 | 2.9 | 1 |
| 10 | 300 | (a16) dispatch, read-write, 16x16 | 2.68 | 2.22 | 8.9 | 1.06 | 3.5 | - |
| 10 | 300 | (a-rows) dispatch, read-write, 4 rows per thread | 3.08 | 2.63 | 10.3 | 1.37 | 4.6 | - |
| 10 | 300 | (b) dispatch, read-only | 2.65 | 2.30 | 8.8 | 0.85 | 2.8 | - |
| 10 | 300 | (c) render pass per dab | 15.82 | 14.09 | 52.7 | 3.54 | 11.8 | - |
| 10 | 900 | (a) dispatch, read-write | 6.15 | 5.12 | 6.8 | 3.86 | 4.3 | 2 |
| 10 | 900 | (a16) dispatch, read-write, 16x16 | 7.49 | 5.93 | 8.3 | 4.60 | 5.1 | - |
| 10 | 900 | (a-rows) dispatch, read-write, 4 rows per thread | 10.25 | 8.01 | 11.4 | 6.49 | 7.2 | - |
| 10 | 900 | (b) dispatch, read-only | 8.78 | 6.39 | 9.8 | 4.03 | 4.5 | - |
| 10 | 900 | (c) render pass per dab | 53.39 | 41.39 | 59.3 | 15.69 | 17.4 | - |
| 10 | 2000 | (a) dispatch, read-write | 8.03 | 7.44 | 4.0 | 5.26 | 2.6 | 2 |
| 10 | 2000 | (a16) dispatch, read-write, 16x16 | 9.70 | 8.80 | 4.9 | 6.52 | 3.3 | - |
| 10 | 2000 | (a-rows) dispatch, read-write, 4 rows per thread | 13.61 | 11.16 | 6.8 | 9.16 | 4.6 | - |
| 10 | 2000 | (b) dispatch, read-only | 10.11 | 8.39 | 5.1 | 5.07 | 2.5 | - |
| 10 | 2000 | (c) render pass per dab | 78.46 | 73.34 | 39.2 | 20.39 | 10.2 | - |
| 1000 | 5 | (a) dispatch, read-write | 7.64 | 6.84 | 1528.8 | 5.73 | 1146.5 | 3 |
| 1000 | 5 | (a16) dispatch, read-write, 16x16 | 7.31 | 6.86 | 1462.2 | 5.40 | 1080.4 | - |
| 1000 | 5 | (a-rows) dispatch, read-write, 4 rows per thread | 6.17 | 5.64 | 1234.5 | 4.32 | 863.9 | - |
| 1000 | 5 | (b) dispatch, read-only | 5.68 | 5.36 | 1135.2 | 3.90 | 780.3 | - |
| 1000 | 5 | (c) render pass per dab | 4.73 | 4.02 | 945.2 | 2.50 | 500.3 | - |
| 1000 | 10 | (a) dispatch, read-write | 12.41 | 11.49 | 1241.4 | 10.02 | 1001.6 | 4 |
| 1000 | 10 | (a16) dispatch, read-write, 16x16 | 12.36 | 11.59 | 1236.0 | 10.21 | 1020.7 | - |
| 1000 | 10 | (a-rows) dispatch, read-write, 4 rows per thread | 9.92 | 9.54 | 991.8 | 7.94 | 794.2 | - |
| 1000 | 10 | (b) dispatch, read-only | 9.09 | 8.71 | 909.0 | 7.32 | 732.3 | - |
| 1000 | 10 | (c) render pass per dab | 7.38 | 7.03 | 737.6 | 4.62 | 461.6 | - |
