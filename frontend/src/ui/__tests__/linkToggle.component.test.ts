// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import LinkToggle from '../LinkToggle.svelte';

const mounted: Array<Record<string, unknown>> = [];

afterEach(() => {
    for (const m of mounted.splice(0)) void unmount(m);
    document.body.innerHTML = '';
});

function render(props: { linked: boolean; onchange: (v: boolean) => void; label: string; bracket?: boolean }) {
    const target = document.createElement('div');
    document.body.append(target);
    mounted.push(mount(LinkToggle, { target, props }) as Record<string, unknown>);
    flushSync();
    return target.querySelector('button')!;
}

describe('link toggle', () => {
    it('an_engaged_link_reads_as_pressed_and_offers_to_unlock', () => {
        const btn = render({ linked: true, onchange: () => {}, label: 'colors to brush' });

        expect(btn.getAttribute('aria-pressed')).toBe('true');
        expect(btn.classList.contains('active')).toBe(true);
        expect(btn.title).toBe('Unlock colors to brush');
    });

    it('a_broken_link_offers_to_lock', () => {
        const btn = render({ linked: false, onchange: () => {}, label: 'aspect ratio' });

        expect(btn.getAttribute('aria-pressed')).toBe('false');
        expect(btn.classList.contains('active')).toBe(false);
        expect(btn.title).toBe('Lock aspect ratio');
    });

    it('clicking_reports_the_opposite_state', () => {
        const onchange = vi.fn();
        const btn = render({ linked: false, onchange, label: 'colors to brush' });

        btn.click();
        flushSync();

        expect(onchange).toHaveBeenCalledWith(true);
    });

    // The connector stubs are what make the chain read as joining the controls
    // on either side of it rather than floating between them.
    it('the_bracket_form_is_opt_in', () => {
        expect(render({ linked: false, onchange: () => {}, label: 'x' }).classList.contains('bracket')).toBe(false);
        expect(render({ linked: false, onchange: () => {}, label: 'x', bracket: true }).classList.contains('bracket')).toBe(true);
    });
});
