// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
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
