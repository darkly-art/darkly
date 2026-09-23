import { defineConfig } from 'vitest/config';

// Vitest owns the unit tests under src/; Playwright owns e2e/. Without this,
// vitest's default glob collects e2e/smoke.test.ts, loads Playwright's test
// runner inside vitest and dies on a duplicate-runner error. They are separate
// suites run by separate commands (npm test vs npm run test:e2e).
export default defineConfig({
    test: {
        include: ['src/**/*.test.ts'],
    },
});
