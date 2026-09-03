import { describe, it, expect } from 'vitest';
// Node builtin; the project intentionally omits @types/node (see
// vite.config.ts and woff2_decode.test.ts). Vitest runs under node, so this
// resolves at runtime.
// @ts-ignore
import { readFileSync } from 'node:fs';

// Regression: UA focus rings kept resurfacing as bright white borders, first
// around whole modals, then on the modal close button. The browser invents
// focus targets on its own (`showModal()` delegates focus to the first
// focusable descendant and forces `:focus-visible` on it, mouse or not), so
// suppressing the ring per-element is whack-a-mole. The global reset must
// kill the UA ring outright; controls that want a ring declare their own
// higher-specificity focus styles. Vitest has no DOM, so assert the
// stylesheet rule directly.
describe('UA focus ring suppression', () => {
    it('reset.css removes the outline from all focused elements', () => {
        const css = readFileSync(
            new URL('../styles/reset.css', import.meta.url),
            'utf8',
        ).replace(/\/\*[\s\S]*?\*\//g, '');

        // A rule whose selector list contains the bare pseudo-classes (not
        // scoped to any element) and whose body sets `outline: none`.
        const rule = /(^|})\s*([^{}]*)\{([^}]*outline:\s*none[^}]*)}/g;
        let selectors: string[] | null = null;
        for (let m = rule.exec(css); m; m = rule.exec(css)) {
            const parts = m[2].split(',').map((s) => s.trim());
            if (parts.includes(':focus') && parts.includes(':focus-visible')) {
                selectors = parts;
                break;
            }
        }
        expect(selectors, 'no global :focus/:focus-visible outline-suppression rule in reset.css').not.toBeNull();
    });
});
