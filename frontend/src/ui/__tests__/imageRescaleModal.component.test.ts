// @vitest-environment jsdom
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

/**
 * Image Size is where an artist overrides the DPI. The field predicts what
 * the engine derives while untouched (keeping the artwork's physical size),
 * and once typed in it sends the number: same dims plus a typed DPI is the
 * DPI-only edit.
 */

const rescaleImage = vi.fn();
const documentDpi = vi.fn(async () => 100);

const app = {
    docW: 100,
    docH: 100,
    engine: { api: { rescaleImage, documentDpi } },
    refreshLayerTree: vi.fn(),
    requestFrame: vi.fn(),
};

vi.mock('../../state/app.svelte', () => ({ app }));

const { imageRescale } = await import('../../state/imageRescale.svelte');
const ImageRescaleModal = (await import('../ImageRescaleModal.svelte')).default;

beforeAll(() => {
    // jsdom implements neither; the modal only needs them not to throw.
    HTMLDialogElement.prototype.showModal = function () { this.open = true; };
    HTMLDialogElement.prototype.close = function () { this.open = false; };
});

let target: HTMLElement;
let instance: Record<string, unknown> | undefined;

/** Mount, open, and let the async `documentDpi` query settle. */
async function render() {
    target = document.createElement('div');
    document.body.append(target);
    instance = mount(ImageRescaleModal, { target, props: {} });
    imageRescale.open = true;
    flushSync();
    await Promise.resolve();
    await Promise.resolve();
    flushSync();
}

function dpiInput(): HTMLInputElement {
    return target.querySelector('.dpi-field input')!;
}

function heightInput(): HTMLInputElement {
    return target.querySelectorAll('.dim-row input')[1] as HTMLInputElement;
}

function type(input: HTMLInputElement, value: string) {
    input.value = value;
    input.dispatchEvent(new Event('input', { bubbles: true }));
    flushSync();
}

function clickRescale() {
    const btn = Array.from(target.querySelectorAll('button')).find(
        b => b.textContent!.trim() === 'Rescale',
    )!;
    btn.click();
    flushSync();
}

beforeEach(() => {
    rescaleImage.mockClear();
    documentDpi.mockClear();
    imageRescale.open = false;
    if (instance) {
        unmount(instance);
        instance = undefined;
    }
});

describe('Image Size: DPI', () => {
    it('shows the document DPI on open', async () => {
        await render();
        expect(Number(dpiInput().value)).toBe(100);
    });

    it('predicts the derived DPI while untouched, and sends null', async () => {
        await render();
        type(heightInput(), '200');
        // Doubling the pixel height at a fixed physical size doubles the DPI.
        expect(Number(dpiInput().value)).toBe(200);

        clickRescale();
        expect(rescaleImage).toHaveBeenCalledWith({
            new_width: 200,
            new_height: 200,
            dpi: null,
        });
    });

    it('sends the typed DPI instead of the prediction', async () => {
        await render();
        type(dpiInput(), '72');
        type(heightInput(), '200');
        // Once touched the field is the artist's, so the prediction stops
        // overwriting it.
        expect(Number(dpiInput().value)).toBe(72);

        clickRescale();
        expect(rescaleImage).toHaveBeenCalledWith({
            new_width: 200,
            new_height: 200,
            dpi: 72,
        });
    });

    it('sends unchanged dims with a typed DPI as the DPI-only edit', async () => {
        await render();
        type(dpiInput(), '300');
        clickRescale();
        expect(rescaleImage).toHaveBeenCalledWith({
            new_width: 100,
            new_height: 100,
            dpi: 300,
        });
    });
});
