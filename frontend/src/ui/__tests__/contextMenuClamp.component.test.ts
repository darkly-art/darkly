// @vitest-environment jsdom
//
// Regression: a context menu opened near a viewport edge must stay on screen.
//
// `ContextMenu` positioned itself at the raw pointer coordinates, so a
// right-click near the bottom or right edge put part of the menu (often the
// destructive entries, which sit last) outside the window with no way to reach
// them. `ColorPopup` and `AddNodeMenu` both clamped; this one did not.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import ContextMenu from '../ContextMenu.svelte';

const mounted: Array<Record<string, unknown>> = [];

const MENU_W = 160;
const MENU_H = 200;

beforeEach(() => {
    window.innerWidth = 1000;
    window.innerHeight = 800;
    // jsdom lays nothing out, so the menu would measure 0x0 and never clamp.
    Object.defineProperty(HTMLElement.prototype, 'offsetWidth', {
        configurable: true,
        get() { return this.classList.contains('context-menu') ? MENU_W : 0; },
    });
    Object.defineProperty(HTMLElement.prototype, 'offsetHeight', {
        configurable: true,
        get() { return this.classList.contains('context-menu') ? MENU_H : 0; },
    });
});

afterEach(() => {
    for (const m of mounted.splice(0)) unmount(m as never);
    document.body.replaceChildren();
});

function open(x: number, y: number) {
    const target = document.createElement('div');
    document.body.append(target);
    mounted.push(
        mount(ContextMenu, {
            target,
            props: { x, y, items: [{ label: 'One', onclick: vi.fn() }], onclose: vi.fn() },
        }) as Record<string, unknown>,
    );
    flushSync();
    return target.querySelector('.context-menu') as HTMLElement;
}

describe('ContextMenu position', () => {
    it('is left alone when the menu already fits', () => {
        const menu = open(100, 120);
        expect(menu.style.left).toBe('100px');
        expect(menu.style.top).toBe('120px');
    });

    it('stays on screen for a right-click near the bottom-right corner', () => {
        const menu = open(980, 780);
        expect(parseFloat(menu.style.left)).toBeLessThanOrEqual(1000 - MENU_W);
        expect(parseFloat(menu.style.top)).toBeLessThanOrEqual(800 - MENU_H);
    });
});
