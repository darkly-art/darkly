// @vitest-environment jsdom
//
// The layer panel's drag affordance, through the mounted drop sites.
//
// jsdom computes no layout and does not implement `getComputedStyle` for
// pseudo-elements, so where the 2px line is painted cannot be asserted here.
// What can be asserted is the declaration the line positions against: the
// shared stylesheet draws it as an absolutely positioned pseudo-element, so
// every drop site has to be its containing block. Vitest processes no CSS (a
// component's styles are not injected, and a `?raw` stylesheet import comes
// back empty), so this file reads the shared stylesheet from disk and appends
// it, and the containing-block assertion reads that stylesheet's own
// declaration. That is the design: a site's scoped CSS supplying it instead
// would fail here on purpose, because a site that has to remember it is how
// the line came to be drawn against the whole list.
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
// @ts-ignore: Node builtins; the project intentionally omits @types/node (see
// vite.config.ts). Vitest runs under node, so these resolve at runtime. The
// path form rather than `new URL(..., import.meta.url)` because under jsdom
// `URL` is jsdom's, which `readFileSync` rejects.
import { readFileSync } from 'node:fs';
// @ts-ignore: as above.
import path from 'node:path';
// @ts-ignore: as above.
import { fileURLToPath } from 'node:url';
import { flushSync, mount, unmount } from 'svelte';
import { DarklyInstance, setActiveInstance } from '../../../state/app.svelte';
import { registerActions } from '../../../actions';
import LayerRow from '../LayerRow.svelte';
import LayerPanel from '../LayerPanel.svelte';
import SpaceDivider from '../SpaceDivider.svelte';
import { rasterNode } from './rowFixtures';

vi.mock('../thumbnails.svelte', () => ({
    THUMB_SIZE: 36,
    getNodeThumbnail: () => 'data:image/png;base64,AA==',
}));

const indicatorCss = readFileSync(
    path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../styles/drop-indicator.css'),
    'utf8',
);

const DIVIDER = 9;
const RASTER = 5;
const MIME = 'application/x-darkly-layers';

const mounted: Array<Record<string, unknown>> = [];
let moveLayers: ReturnType<typeof vi.fn>;
let instance: DarklyInstance;
const app = () => instance;

beforeAll(() => {
    registerActions();
});

beforeEach(() => {
    const style = document.createElement('style');
    style.textContent = indicatorCss;
    document.head.append(style);

    moveLayers = vi.fn().mockResolvedValue(0);
    const inst = new DarklyInstance();
    instance = inst;
    inst.engine = {
        api: { moveLayers, getLayerTree: vi.fn().mockResolvedValue({ layers: [] }) },
    } as never;
    inst.requestFrame = vi.fn();
    inst.refreshLayerTree = vi.fn() as never;
    // Viewport space empty: the divider is the first row.
    inst.layerTree = [{ id: DIVIDER, type: 'divider' }, rasterNode({ id: RASTER })] as never;
    setActiveInstance(inst);
});

afterEach(() => {
    for (const instance of mounted.splice(0)) unmount(instance as never);
    document.body.replaceChildren();
    document.head.replaceChildren();
    setActiveInstance(null);
});

function render(component: never, props: Record<string, unknown>): HTMLElement {
    const target = document.createElement('div');
    document.body.append(target);
    mounted.push(mount(component, { target, props } as never) as Record<string, unknown>);
    flushSync();
    return target;
}

/** A canned box, since jsdom lays nothing out. */
function stubRect(el: HTMLElement, height: number) {
    el.getBoundingClientRect = () =>
        ({ left: 0, top: 0, right: 200, bottom: height, width: 200, height, x: 0, y: 0, toJSON() {} }) as DOMRect;
}

/** jsdom has no `DragEvent` or `DataTransfer`, so a drag event is a mouse
 *  event carrying a hand-built transfer. */
function drag(el: HTMLElement, type: string, clientY: number, payload: number[] = [RASTER]) {
    const e = new MouseEvent(type, { bubbles: true, cancelable: true, clientX: 8, clientY });
    Object.defineProperty(e, 'dataTransfer', {
        value: {
            dropEffect: '',
            effectAllowed: '',
            setData() {},
            getData: (mime: string) => (mime === MIME ? JSON.stringify(payload) : ''),
        },
    });
    el.dispatchEvent(e);
    flushSync();
}

async function settle() {
    await Promise.resolve();
    await Promise.resolve();
}

describe('drop sites', () => {
    it('every drop site is the containing block its indicator positions against', () => {
        const divider = render(SpaceDivider as never, { divider: { id: DIVIDER }, empty: true })
            .querySelector('.divider') as HTMLElement;
        const row = render(LayerRow as never, { node: rasterNode({ id: RASTER }), onupdate: vi.fn() })
            .querySelector('.layer-row') as HTMLElement;

        expect(getComputedStyle(divider).position).toBe('relative');
        expect(getComputedStyle(row).position).toBe('relative');
    });

    // Characterization: the divider is a leaf row to the drop model. Its upper
    // half is the gap above it (viewport space), its lower half the gap below
    // it (the top of canvas space). The hit area exists to make both reachable;
    // turning the divider into a single-gap site would fail here.
    it('the divider is a leaf row to a drag', async () => {
        const divider = render(SpaceDivider as never, { divider: { id: DIVIDER }, empty: true })
            .querySelector('.divider') as HTMLElement;
        stubRect(divider, 24);

        drag(divider, 'dragover', 4);
        expect(divider.classList.contains('drop-above')).toBe(true);
        expect(divider.classList.contains('drop-below')).toBe(false);
        drag(divider, 'drop', 4);
        await settle();
        expect(moveLayers).toHaveBeenLastCalledWith({
            ids: [RASTER],
            target: { target_type: 'after', target_id: DIVIDER },
        });

        drag(divider, 'dragover', 20);
        expect(divider.classList.contains('drop-below')).toBe(true);
        expect(divider.classList.contains('drop-above')).toBe(false);
        drag(divider, 'drop', 20);
        await settle();
        expect(moveLayers).toHaveBeenLastCalledWith({
            ids: [RASTER],
            target: { target_type: 'before', target_id: DIVIDER },
        });
    });

    // Feature: the list's empty area is a drop site whose top edge is the gap
    // below the last row, so its line draws there and not across the top of
    // the list.
    it('the list tail is a drop site and the list is not', async () => {
        const BOTTOM = 6;
        app().layerTree = [
            { id: DIVIDER, type: 'divider' },
            rasterNode({ id: RASTER }),
            rasterNode({ id: BOTTOM }),
        ] as never;
        const target = render(LayerPanel as never, {});
        const list = target.querySelector('.layer-list') as HTMLElement;
        const tail = list.querySelector(':scope > .drop-tail') as HTMLElement;

        expect(list.classList.contains('drop-site')).toBe(false);
        expect(tail).not.toBeNull();
        expect(tail.classList.contains('drop-site')).toBe(true);

        stubRect(tail, 24);
        drag(tail, 'dragover', 12);
        expect(tail.classList.contains('drop-above')).toBe(true);
        drag(tail, 'drop', 12);
        await settle();
        expect(moveLayers).toHaveBeenLastCalledWith({
            ids: [RASTER],
            target: { target_type: 'before', target_id: BOTTOM },
        });
    });
});
