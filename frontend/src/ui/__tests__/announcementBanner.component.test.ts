// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import AnnouncementBanner from '../AnnouncementBanner.svelte';

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

function render(props: { html: string; ready?: boolean; ondismiss?: () => void }) {
    const host = target();
    mounted.push(
        mount(AnnouncementBanner, {
            target: host,
            props: { ready: true, ondismiss: () => {}, ...props },
        }) as Record<string, unknown>,
    );
    flushSync();
    return host.querySelector('aside')!;
}

describe('the announcement banner', () => {
    it('renders the markup the build supplied, rather than escaping it', () => {
        const banner = render({ html: '<b>Darkly is in beta.</b>' });

        expect(banner.querySelector('b')?.textContent).toBe('Darkly is in beta.');
    });

    it('sends the build author\'s plain links to a new tab, opener severed', () => {
        // A same-tab navigation would tear down the editor and its unsaved
        // documents, so the app hardens the anchors rather than trusting
        // whoever wrote DARKLY_BANNER to remember the attributes.
        const banner = render({
            html: 'Report bugs on <a href="https://discord.gg/x">Discord</a>.',
        });

        const a = banner.querySelector('a')!;
        expect(a.target).toBe('_blank');
        expect(a.rel).toBe('noopener noreferrer');
    });

    it('stays shut while the app is still coming up', () => {
        vi.useFakeTimers();
        try {
            // It opens from zero height, so arriving mid-boot would reflow the
            // canvas while the engine is still setting itself up.
            const banner = render({ html: 'beta', ready: false });

            vi.runAllTimers();
            flushSync();

            expect(banner.classList.contains('shown')).toBe(false);
        } finally {
            vi.useRealTimers();
        }
    });

    it('opens a beat after the app reports itself up', () => {
        vi.useFakeTimers();
        try {
            const banner = render({ html: 'beta', ready: true });
            expect(banner.classList.contains('shown')).toBe(false);

            vi.runAllTimers();
            flushSync();

            expect(banner.classList.contains('shown')).toBe(true);
        } finally {
            vi.useRealTimers();
        }
    });

    it('dismisses through its close control', () => {
        const ondismiss = vi.fn();
        const banner = render({ html: 'beta', ondismiss });

        banner.querySelector('button')!.click();

        expect(ondismiss).toHaveBeenCalledTimes(1);
    });

    it('keeps its close control reachable from the keyboard without firing a canvas hotkey', () => {
        const banner = render({ html: 'beta' });
        const button = banner.querySelector('button')!;

        // An explicit tabindex is what survives `suppressButtonKeyboardFocus`,
        // which otherwise stamps -1 on every button in the app. Dismissing is
        // this banner's only affordance, so losing it locks a keyboard-only
        // reader out for good.
        expect(button.getAttribute('tabindex')).toBe('0');

        // The flip side: activation keys must stop here rather than also
        // reaching the window-level hotkey handler.
        const reachedWindow = vi.fn();
        window.addEventListener('keydown', reachedWindow);
        button.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
        window.removeEventListener('keydown', reachedWindow);

        expect(reachedWindow).not.toHaveBeenCalled();
    });
});
