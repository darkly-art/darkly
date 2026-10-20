# dispatch_cost_bench at `9d824e90e8`

Adapter: Intel(R) Graphics (RPL-U) (Vulkan, driver Intel open-source Mesa driver Mesa 26.2.3-arch1.1). Target 3840x2160. 20 iterations per cell, p50 reported. Pass timestamps: yes.

| radius_px | N | shape | wall p50 (ms) | wall min (ms) | wall per dab, p50 (us) | gpu p50 (ms) | gpu per dab, p50 (us) | parity max diff (LSB) |
|---:|---:|---|---:|---:|---:|---:|---:|---:|
| 1.5 | 300 | (a) dispatch, read-write | 4.65 | 3.62 | 15.5 | 2.86 | 9.5 | 1 |
| 1.5 | 300 | (b) dispatch, read-only | 5.03 | 3.52 | 16.8 | 2.77 | 9.2 | - |
| 1.5 | 300 | (c) render pass per dab | 28.12 | 19.21 | 93.7 | 11.54 | 38.5 | - |
| 1.5 | 900 | (a) dispatch, read-write | 6.46 | 5.60 | 7.2 | 4.69 | 5.2 | 1 |
| 1.5 | 900 | (b) dispatch, read-only | 6.97 | 6.07 | 7.7 | 4.56 | 5.1 | - |
| 1.5 | 900 | (c) render pass per dab | 49.59 | 43.34 | 55.1 | 19.03 | 21.1 | - |
| 1.5 | 2000 | (a) dispatch, read-write | 7.82 | 7.16 | 3.9 | 5.48 | 2.7 | 1 |
| 1.5 | 2000 | (b) dispatch, read-only | 8.54 | 7.02 | 4.3 | 5.50 | 2.7 | - |
| 1.5 | 2000 | (c) render pass per dab | 73.96 | 67.20 | 37.0 | 21.78 | 10.9 | - |
| 10 | 300 | (a) dispatch, read-write | 2.47 | 2.01 | 8.2 | 1.09 | 3.6 | 1 |
| 10 | 300 | (b) dispatch, read-only | 2.66 | 2.08 | 8.9 | 1.07 | 3.6 | - |
| 10 | 300 | (c) render pass per dab | 15.42 | 12.44 | 51.4 | 4.25 | 14.2 | - |
| 10 | 900 | (a) dispatch, read-write | 6.59 | 5.64 | 7.3 | 4.25 | 4.7 | 2 |
| 10 | 900 | (b) dispatch, read-only | 7.54 | 5.37 | 8.4 | 4.56 | 5.1 | - |
| 10 | 900 | (c) render pass per dab | 49.61 | 42.92 | 55.1 | 16.74 | 18.6 | - |
| 10 | 2000 | (a) dispatch, read-write | 8.20 | 7.34 | 4.1 | 5.67 | 2.8 | 2 |
| 10 | 2000 | (b) dispatch, read-only | 7.91 | 7.30 | 4.0 | 5.33 | 2.7 | - |
| 10 | 2000 | (c) render pass per dab | 73.72 | 66.97 | 36.9 | 21.24 | 10.6 | - |
