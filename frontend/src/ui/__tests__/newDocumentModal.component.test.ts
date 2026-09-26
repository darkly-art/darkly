// @vitest-environment jsdom
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

/**
 * Auto DPI is what makes a brush behave the same on every new document, so
 * the checkbox is on by default and the dialog sends `dpi: null` unless the
 * artist deliberately takes the wheel.
 */

const opened: Array<{ width: number; height: number; dpi?: number | null }> = [];

const shell = {
    open: vi.fn((_name?: string, dims?: any) => {
        opened.push(dims);
        return { onHandleReady: null, selectLayer: vi.fn() };
    }),
};

const configValues: Record<string, number | string> = {
    'canvas.width': 1920,
    'canvas.height': 1080,
    'canvas.dpi': 300,
};

vi.mock('../../multi_tab/shell.svelte', () => ({ shell }));
vi.mock('../../config/store.svelte', () => ({
    config: { get: (key: string) => configValues[key] },
}));
vi.mock('../../state/app.svelte', () => ({
    app: { refreshLayerTree: vi.fn(), requestFrame: vi.fn() },
}));
// Reading the clipboard needs a real permission prompt; the peek is not what
// these cases are about.
vi.mock('../../clipboard', () => ({ readImageFromClipboard: async () => null }));

const { newDocument } = await import('../../state/newDocument.svelte');
const NewDocumentModal = (await import('../NewDocumentModal.svelte')).default;

beforeAll(() => {
    // jsdom implements neither; the modal only needs them not to throw.
    HTMLDialogElement.prototype.showModal = function () { this.open = true; };
    HTMLDialogElement.prototype.close = function () { this.open = false; };
});

let target: HTMLElement;
let instance: Record<string, unknown> | undefined;

function render() {
    target = document.createElement('div');
    document.body.append(target);
    instance = mount(NewDocumentModal, { target, props: {} });
    newDocument.open = true;
    flushSync();
}

function dpiInput(): HTMLInputElement | null {
    return target.querySelector('.dpi-field input');
}

function autoCheckbox(): HTMLInputElement {
    return target.querySelector('.check-row input[type="checkbox"]')!;
}

function clickCreate() {
    const create = Array.from(target.querySelectorAll('button')).find(
        b => b.textContent!.trim() === 'Create',
    )!;
    create.click();
    flushSync();
}

beforeEach(() => {
    opened.length = 0;
    shell.open.mockClear();
    newDocument.open = false;
    if (instance) {
        unmount(instance);
        instance = undefined;
    }
});

describe('New Document: auto DPI', () => {
    it('is checked on open, and hides the manual field', () => {
        render();
        expect(autoCheckbox().checked).toBe(true);
        expect(dpiInput()).toBeNull();
    });

    it('sends a null DPI so the engine derives it from the pixel size', () => {
        render();
        clickCreate();
        expect(opened).toEqual([{ width: 1920, height: 1080, dpi: null }]);
    });

    it('reveals a field prefilled from canvas.dpi when unchecked', () => {
        render();
        autoCheckbox().click();
        flushSync();
        const field = dpiInput();
        expect(field).not.toBeNull();
        expect(Number(field!.value)).toBe(300);
    });

    it('sends the artist value once they have taken the wheel', () => {
        render();
        autoCheckbox().click();
        flushSync();
        const field = dpiInput()!;
        field.value = '150';
        field.dispatchEvent(new Event('input', { bubbles: true }));
        flushSync();
        clickCreate();
        expect(opened).toEqual([{ width: 1920, height: 1080, dpi: 150 }]);
    });

    it('resets to auto when the dialog is reopened', () => {
        render();
        autoCheckbox().click();
        flushSync();
        expect(dpiInput()).not.toBeNull();

        newDocument.open = false;
        flushSync();
        newDocument.open = true;
        flushSync();

        expect(autoCheckbox().checked).toBe(true);
        expect(dpiInput()).toBeNull();
    });
});
