// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

import Tabs from '../Tabs.svelte';

const TABS = [
    { id: 'a', label: 'Alpha' },
    { id: 'b', label: 'Beta' },
    { id: 'c', label: 'Gamma' },
];

const mounted: Array<Record<string, unknown>> = [];

afterEach(() => {
    for (const m of mounted.splice(0)) void unmount(m);
    document.body.innerHTML = '';
});

function render(active: string, orientation: 'row' | 'column' = 'row') {
    const onselect = vi.fn();
    const target = document.createElement('div');
    document.body.append(target);
    mounted.push(
        mount(Tabs, { target, props: { tabs: TABS, active, onselect, orientation } }) as Record<
            string,
            unknown
        >,
    );
    flushSync();
    return { target, onselect };
}

function tabs(target: HTMLElement): HTMLButtonElement[] {
    return Array.from(target.querySelectorAll<HTMLButtonElement>('[role=tab]'));
}

function press(el: HTMLElement, key: string) {
    el.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
    flushSync();
}

describe('Tabs', () => {
    it('marks_the_active_tab_and_nothing_else', () => {
        const { target } = render('b');
        const [a, b, c] = tabs(target);
        expect(tabs(target).map((t) => t.textContent)).toEqual(['Alpha', 'Beta', 'Gamma']);
        expect(b.getAttribute('aria-selected')).toBe('true');
        expect(b.classList.contains('active')).toBe(true);
        expect(a.getAttribute('aria-selected')).toBe('false');
        expect(c.classList.contains('active')).toBe(false);
        // Only the active tab is in the tab order; arrows walk the rest.
        expect(b.tabIndex).toBe(0);
        expect(a.tabIndex).toBe(-1);
    });

    it('click_reports_the_tab_id', () => {
        const { target, onselect } = render('a');
        tabs(target)[2].click();
        expect(onselect).toHaveBeenCalledWith('c');
    });

    it('arrows_walk_the_strip_axis_and_stop_at_the_ends', () => {
        const { target, onselect } = render('b');
        const [a, b] = tabs(target);
        press(b, 'ArrowRight');
        expect(onselect).toHaveBeenLastCalledWith('c');
        press(b, 'ArrowLeft');
        expect(onselect).toHaveBeenLastCalledWith('a');
        // Cross-axis keys are not the strip's business in a row.
        press(b, 'ArrowDown');
        expect(onselect).toHaveBeenCalledTimes(2);
        // The first tab has nowhere to go left.
        press(a, 'ArrowLeft');
        expect(onselect).toHaveBeenCalledTimes(2);
    });

    it('a_column_walks_with_up_and_down', () => {
        const { target, onselect } = render('a', 'column');
        const [a] = tabs(target);
        press(a, 'ArrowDown');
        expect(onselect).toHaveBeenLastCalledWith('b');
        press(a, 'ArrowRight');
        expect(onselect).toHaveBeenCalledTimes(1);
        expect(target.querySelector('.tab-strip')?.classList.contains('column')).toBe(true);
    });
});
