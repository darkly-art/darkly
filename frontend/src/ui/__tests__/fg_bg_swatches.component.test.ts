// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

// The swatches read and write the focused instance's colors and the two
// color prefs through the config store, which is WASM-backed in production.
const { fakeConfig } = vi.hoisted(() => ({
    fakeConfig: { values: {} as Record<string, unknown>, get(k: string) { return this.values[k]; } },
}));
vi.mock('../../config/store.svelte', async (importOriginal) => ({
    ...(await importOriginal<object>()),
    config: fakeConfig,
    tooltipForAction: (label: string) => label,
}));

import { DarklyInstance, setActiveInstance } from '../../state/app.svelte';
import { RECIPES, deployMode } from '../../state/freshDocument';
import FgBgSwatches from '../color/FgBgSwatches.svelte';

beforeEach(() => {
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(
        () =>
            ({
                createImageData: (w: number, h: number) => ({ data: new Uint8ClampedArray(w * h * 4) }),
                putImageData() {},
            }) as unknown as ReturnType<HTMLCanvasElement['getContext']>,
    );
});

let inst: DarklyInstance;
const mounted: Array<Record<string, unknown>> = [];

beforeEach(() => {
    inst = new DarklyInstance();
    setActiveInstance(inst);
    fakeConfig.values = {};
});
afterEach(() => {
    for (const m of mounted.splice(0)) void unmount(m);
    document.body.innerHTML = '';
    setActiveInstance(null);
    vi.restoreAllMocks();
});

function render() {
    const target = document.createElement('div');
    document.body.append(target);
    mounted.push(mount(FgBgSwatches, { target, props: { mode: 'popup' } }) as Record<string, unknown>);
    flushSync();
    return target;
}

function click(target: HTMLElement, selector: string) {
    const el = target.querySelector<HTMLElement>(selector);
    if (!el) throw new Error(`missing ${selector}`);
    el.click();
    flushSync();
}

describe('foreground/background swatches', () => {
    it('swap_exchanges_the_pair', () => {
        inst.foreground = { r: 1, g: 2, b: 3, a: 255 };
        inst.background = { r: 9, g: 8, b: 7, a: 255 };
        const target = render();

        click(target, '.swap');

        expect(inst.foreground).toEqual({ r: 9, g: 8, b: 7, a: 255 });
        expect(inst.background).toEqual({ r: 1, g: 2, b: 3, a: 255 });
    });

    it('swapping_twice_returns_the_original_pair', () => {
        inst.foreground = { r: 1, g: 2, b: 3, a: 255 };
        inst.background = { r: 9, g: 8, b: 7, a: 255 };
        const target = render();

        click(target, '.swap');
        click(target, '.swap');

        expect(inst.foreground).toEqual({ r: 1, g: 2, b: 3, a: 255 });
        expect(inst.background).toEqual({ r: 9, g: 8, b: 7, a: 255 });
        // The swatches repaint from the pair, so a broken reactive graph shows
        // up here as a stale background.
        expect(target.querySelector<HTMLElement>('.swatch.fg')!.style.background).toBe('rgb(1, 2, 3)');
        expect(target.querySelector<HTMLElement>('.swatch.bg')!.style.background).toBe('rgb(9, 8, 7)');
    });

    it('reset_uses_the_configured_defaults', () => {
        fakeConfig.values = { 'colors.defaultForeground': '#112233', 'colors.defaultBackground': '#445566' };
        inst.foreground = { r: 1, g: 2, b: 3, a: 255 };
        const target = render();

        click(target, '.reset');

        expect(inst.foreground).toEqual({ r: 0x11, g: 0x22, b: 0x33, a: 255 });
        expect(inst.background).toEqual({ r: 0x44, g: 0x55, b: 0x66, a: 255 });
    });

    it('reset_falls_back_to_the_fresh_document_pair_when_a_pref_is_unset_or_malformed', () => {
        fakeConfig.values = { 'colors.defaultForeground': 'not a color' };
        inst.foreground = { r: 1, g: 2, b: 3, a: 255 };
        inst.background = { r: 4, g: 5, b: 6, a: 255 };
        const target = render();

        click(target, '.reset');

        expect(inst.foreground).toEqual(RECIPES[deployMode].foreground);
        expect(inst.background).toEqual(RECIPES[deployMode].background);
    });

    it('a_swatch_opens_the_popup_for_that_swatch_and_an_outside_press_closes_it', () => {
        const target = render();
        expect(target.querySelector('.color-popup')).toBeNull();

        click(target, '.swatch.bg');
        const popup = target.querySelector('.color-popup');
        expect(popup).not.toBeNull();
        expect(popup!.getAttribute('data-keep-open')).toBe('fg-bg-color');
        expect(popup!.querySelector('.wheel')).not.toBeNull();

        // A press on the popup itself keeps it open; one anywhere else closes it.
        popup!.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
        flushSync();
        expect(target.querySelector('.color-popup')).not.toBeNull();
        document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
        flushSync();
        expect(target.querySelector('.color-popup')).toBeNull();
    });

    it('escape_closes_the_popup', () => {
        const target = render();
        click(target, '.swatch.fg');
        expect(target.querySelector('.color-popup')).not.toBeNull();

        window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
        flushSync();

        expect(target.querySelector('.color-popup')).toBeNull();
    });
});
