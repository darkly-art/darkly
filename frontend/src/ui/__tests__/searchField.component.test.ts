// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
// Node builtins; the project intentionally omits @types/node (see
// vite.config.ts and dialogFocusRing.test.ts). Vitest runs under node.
// @ts-ignore
import { readFileSync, readdirSync } from 'node:fs';
// @ts-ignore
import { fileURLToPath } from 'node:url';
import SearchField from '../SearchField.svelte';
import SearchFieldHarness from './SearchFieldHarness.test.svelte';

const mounted: Array<Record<string, unknown>> = [];

afterEach(() => {
    for (const m of mounted.splice(0)) void unmount(m);
    document.body.innerHTML = '';
});

function target(): HTMLElement {
    const el = document.createElement('div');
    document.body.append(el);
    return el;
}

function render(props: { placeholder?: string; onkeydown?: (e: KeyboardEvent) => void } = {}) {
    const host = target();
    mounted.push(mount(SearchField, { target: host, props }) as Record<string, unknown>);
    flushSync();
    return host.querySelector('input')!;
}

describe('the search field', () => {
    it('names itself by its placeholder, since the magnifier carries no text', () => {
        const input = render({ placeholder: 'Search fonts…' });

        expect(input.type).toBe('search');
        expect(input.placeholder).toBe('Search fonts…');
        expect(input.getAttribute('aria-label')).toBe('Search fonts…');
    });

    it('passes keystrokes to the caller that owns the list below it', () => {
        const onkeydown = vi.fn();
        const input = render({ onkeydown });

        input.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));

        expect(onkeydown).toHaveBeenCalledOnce();
    });

    it('reports the typed query, and the input, back to its caller', () => {
        const onquery = vi.fn();
        const host = target();
        const harness = mount(SearchFieldHarness, { target: host, props: { onquery } }) as {
            input: () => HTMLInputElement | null;
        };
        mounted.push(harness as unknown as Record<string, unknown>);
        flushSync();

        const input = harness.input();
        expect(input).toBe(host.querySelector('input'));

        input!.value = 'grain';
        input!.dispatchEvent(new Event('input', { bubbles: true }));
        flushSync();

        expect(onquery).toHaveBeenLastCalledWith('grain');
    });
});

// The whole point of the component: a second hand-rolled search box is how the
// ones that existed before drifted into as many different looks as there were
// call sites.
describe('search boxes across the app', () => {
    it('are all this one component', () => {
        const ui = fileURLToPath(new URL('..', import.meta.url));
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
