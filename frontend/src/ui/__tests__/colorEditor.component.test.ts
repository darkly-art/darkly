// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

// The color editor (the widget the swatch popup and the docked Color panel
// both host): its tabs, and what a paint host gets that a generic color field
// does not. The jitter tab reads and writes the `colors.jitter*` prefs through
// the config store, which is WASM-backed in production; the panel host reads
// the focused instance's colors.
vi.mock('../../config/store.svelte', async (importOriginal) => ({
    ...(await importOriginal<object>()),
    config: (await import('../../__tests__/fakeConfig.svelte')).fakeConfig,
    tooltipForAction: (label: string) => label,
}));

import { fakeConfig } from '../../__tests__/fakeConfig.svelte';
import { DarklyInstance, setActiveInstance } from '../../state/app.svelte';
import type { Color } from '../../lib/color';
import ColorEditor from '../color/ColorEditor.svelte';
import ColorPanel from '../color/ColorPanel.svelte';
import { pointForSv, wheelGeometry } from '../color/wheel_model';

beforeEach(() => {
    Element.prototype.setPointerCapture = () => {};
    Element.prototype.hasPointerCapture = () => false;
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(
        () =>
            ({
                createImageData: (w: number, h: number) => ({ data: new Uint8ClampedArray(w * h * 4) }),
                putImageData() {},
            }) as unknown as ReturnType<HTMLCanvasElement['getContext']>,
    );
});

const mounted: Array<Record<string, unknown>> = [];
let inst: DarklyInstance;

beforeEach(() => {
    inst = new DarklyInstance();
    setActiveInstance(inst);
    fakeConfig.reset();
});
afterEach(() => {
    for (const m of mounted.splice(0)) void unmount(m);
    document.body.innerHTML = '';
    setActiveInstance(null);
    vi.restoreAllMocks();
});

const RED: Color = { r: 255, g: 0, b: 0, a: 255 };

function render(paint = true) {
    const target = document.createElement('div');
    document.body.append(target);
    const props = { value: RED, oninput: () => {}, onchange: () => {}, size: 200, paint };
    mounted.push(mount(ColorEditor, { target, props }) as Record<string, unknown>);
    flushSync();
    return target;
}

function renderPanel() {
    const target = document.createElement('div');
    document.body.append(target);
    mounted.push(mount(ColorPanel, { target, props: {} }) as Record<string, unknown>);
    flushSync();
    return target;
}

function clickTab(target: HTMLElement, label: string) {
    const tab = Array.from(target.querySelectorAll<HTMLButtonElement>('[role=tab]')).find(
        (t) => t.textContent === label,
    );
    if (!tab) throw new Error(`no tab ${label}`);
    tab.click();
    flushSync();
}

function scrubs(target: HTMLElement): HTMLElement[] {
    return Array.from(target.querySelectorAll<HTMLElement>('.scrub'));
}

function scrubValue(scrub: HTMLElement): string {
    return scrub.querySelector('.bar-control-value')?.textContent ?? '';
}

describe('color editor', () => {
    it('opens_on_the_wheel_with_no_jitter_controls', () => {
        const target = render();
        expect(target.querySelector('[role=tab][aria-selected=true]')?.textContent).toBe('Wheel');
        expect(target.querySelector('canvas')).not.toBeNull();
        expect(scrubs(target)).toHaveLength(0);
    });

    it('the_jitter_tab_shows_the_three_amplitudes_from_the_prefs', () => {
        fakeConfig.set('colors.jitterHue', 25);
        fakeConfig.set('colors.jitterSaturation', 0.5);
        const target = render();
        clickTab(target, 'Jitter');

        expect(target.querySelector('canvas')).toBeNull();
        const controls = scrubs(target);
        expect(controls.map((s) => s.querySelector('.bar-control-label')?.textContent)).toEqual([
            'Hue',
            'Saturation',
            'Value',
        ]);
        expect(controls.map(scrubValue)).toEqual(['25°', '50%', '0%']);
    });

    it('resetting_a_scrub_writes_zero_to_its_pref', () => {
        fakeConfig.set('colors.jitterHue', 25);
        const target = render();
        clickTab(target, 'Jitter');

        scrubs(target)[0].dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
        flushSync();

        expect(fakeConfig.get('colors.jitterHue')).toBe(0);
        expect(scrubValue(scrubs(target)[0])).toBe('0°');
    });

    it('the_wheel_tab_comes_back', () => {
        const target = render();
        clickTab(target, 'Jitter');
        clickTab(target, 'Wheel');
        expect(target.querySelector('canvas')).not.toBeNull();
        expect(scrubs(target)).toHaveLength(0);
    });

    it('without_paint_there_is_no_tab_strip_and_no_jitter', () => {
        const target = render(false);
        expect(target.querySelector('[role=tab]')).toBeNull();
        expect(target.querySelector('canvas')).not.toBeNull();
        expect(target.querySelector('.hex')).not.toBeNull();
        expect(scrubs(target)).toHaveLength(0);
    });
});

describe('color panel', () => {
    it('hosts_the_editor_with_the_jitter_tab_and_edits_the_selected_swatch', () => {
        inst.foreground = { r: 0, g: 255, b: 0, a: 255 };
        inst.background = { r: 0, g: 0, b: 128, a: 255 };
        const target = renderPanel();
        expect(target.querySelector('.swatches')).not.toBeNull();
        expect(
            Array.from(target.querySelectorAll('[role=tab]')).map((t) => t.textContent),
        ).toEqual(['Wheel', 'Jitter']);

        // Select the navy background, then press the triangle's hue corner:
        // full saturation and value at navy's hue is pure blue.
        target.querySelector<HTMLElement>('.swatch.bg')!.click();
        flushSync();
        const wheel = target.querySelector('.wheel')!;
        wheel.getBoundingClientRect = () =>
            ({ left: 0, top: 0, right: 220, bottom: 220, width: 220, height: 220, x: 0, y: 0, toJSON() {} }) as DOMRect;
        const p = pointForSv(wheelGeometry(220), 240, 1, 1);
        wheel.dispatchEvent(
            new PointerEvent('pointerdown', { clientX: p.x, clientY: p.y, pointerId: 1, button: 0, bubbles: true }),
        );
        flushSync();

        expect(inst.background).toEqual({ r: 0, g: 0, b: 255, a: 255 });
        expect(inst.foreground).toEqual({ r: 0, g: 255, b: 0, a: 255 });
    });
});
