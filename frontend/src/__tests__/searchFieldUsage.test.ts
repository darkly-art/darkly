import { describe, it, expect } from 'vitest';
// Node builtins; the project intentionally omits @types/node (see
// vite.config.ts and dialogFocusRing.test.ts). Vitest runs under node.
// @ts-ignore
import { readFileSync, readdirSync } from 'node:fs';
// @ts-ignore
import { fileURLToPath } from 'node:url';

// The whole point of the component: a second hand-rolled search box is how the
// ones that existed before drifted into as many different looks as there were
// call sites.
//
// A repo scan rather than a component test, so it lives beside the other one
// (`dialogFocusRing.test.ts`) and takes the default node environment. Under
// jsdom `import.meta.url` is an `http:` URL and `fileURLToPath` rejects it.
describe('search boxes across the app', () => {
    it('are all this one component', () => {
        const ui = fileURLToPath(new URL('../ui', import.meta.url));
        const offenders: string[] = [];

        const walk = (dir: string) => {
            for (const e of readdirSync(dir, { withFileTypes: true })) {
                const path = `${dir}/${e.name}`;
                if (e.isDirectory()) {
                    if (e.name !== '__tests__') walk(path);
                } else if (e.name.endsWith('.svelte') && e.name !== 'SearchField.svelte') {
                    if (/type=["']search["']/.test(readFileSync(path, 'utf8'))) {
                        offenders.push(path.slice(ui.length));
                    }
                }
            }
        };
        walk(ui);

        expect(offenders, 'use SearchField instead of a bare search input').toEqual([]);
    });
});
