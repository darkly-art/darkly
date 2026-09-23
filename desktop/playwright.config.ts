import { defineConfig } from '@playwright/test';

export default defineConfig({
    testDir: './e2e',
    // Generous per-test timeout: WASM init + window paint can take >10s
    // on a cold start, especially under software rendering / xvfb.
    timeout: 90_000,
    reporter: 'list',
    // Only ever run one Electron instance at a time: they all fight for
    // the same userData dir and display in CI.
    fullyParallel: false,
    workers: 1,
});
