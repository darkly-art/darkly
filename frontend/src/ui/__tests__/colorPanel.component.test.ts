// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

// The panel's wheel reads the focused instance's colours and the jitter tab
// reads and writes the `colors.jitter*` prefs through the config store, which
// is WASM-backed in production.
vi.mock('../../config/store.svelte', async (importOriginal) => ({
    ...(await importOriginal<object>()),
    config: (await import('../../__tests__/fakeConfig.svelte')).fakeConfig,
    tooltipForAction: (label: string) => label,
}));

import { fakeConfig } from '../../__tests__/fakeConfig.svelte';
import { DarklyInstance, setActiveInstance } from '../../state/app.svelte';
import ColorPanel from '../color/ColorPanel.svelte';

beforeEach(() => {
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(
        () =>
            ({
                createImageData: (w: number, h: number) => ({ data: new Uint8ClampedArray(w * h * 4) }),
                putImageData() {},
            }) as unknown as ReturnType<HTMLCanvasElement['getContext']>,
    );
});

const mounted: Array<Record<string, unknown>> = [];

beforeEach(() => {
    setActiveInstance(new DarklyInstance());
    fakeConfig.reset();
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

describe('colour panel', () => {
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
});
