// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import ColorInput from '../ColorInput.svelte';

// jsdom has no 2D canvas; the popup's wheel needs one if it ever opens.
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

afterEach(() => {
    for (const m of mounted.splice(0)) void unmount(m);
    document.body.innerHTML = '';
    vi.restoreAllMocks();
});

function render(value: string | undefined) {
    const target = document.createElement('div');
    document.body.append(target);
    mounted.push(
        mount(ColorInput, { target, props: { value: value as string, onchange: () => {} } }) as Record<string, unknown>,
    );
    flushSync();
    return target;
}

describe('color input', () => {
    // A pref whose value is unset arrives here as `undefined`: `config.get`
    // is untyped and the settings row casts. Throwing on it took down the
    // whole reactive graph, so unrelated controls stopped updating too.
    it('an_unset_value_renders_as_black_without_throwing', () => {
        const target = render(undefined);

        expect(target.querySelector('.swatch')).not.toBeNull();
        expect(target.querySelector<HTMLInputElement>('.hex')!.value).toBe('');
    });

    it('a_malformed_value_renders_as_black_without_throwing', () => {
        const target = render('rebeccapurple');

        expect(target.querySelector('.swatch')).not.toBeNull();
        expect(target.querySelector<HTMLInputElement>('.hex')!.value).toBe('rebeccapurple');
    });

    it('a_valid_value_paints_the_swatch', () => {
        const target = render('#3355ff');

        expect(target.querySelector<HTMLElement>('.swatch')!.style.background).toBe('rgb(51, 85, 255)');
    });
});
