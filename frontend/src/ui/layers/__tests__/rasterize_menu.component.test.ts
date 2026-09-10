// @vitest-environment jsdom
//
// The layer row's Rasterize entry, end to end: right-click → click → engine op.
//
// Regression: the entry rendered for a layer whose pixels are generated, but
// its click handler carried its own copy of the "should this be offered?" rule
// and still bailed unless the layer had a mask. Clicking it dispatched
// nothing: no engine call, no error, nothing in the console. A test of the
// predicate alone can't see that; this mounts the row and clicks the entry.
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount, type ComponentProps } from 'svelte';
import { DarklyInstance, setActiveInstance } from '../../../state/app.svelte';
import { registerActions } from '../../../actions';
import LayerItem from '../LayerItem.svelte';

vi.mock('../thumbnails.svelte', () => ({
    THUMB_SIZE: 36,
    getNodeThumbnail: () => 'data:image/png;base64,AA==',
}));

const mounted: Array<Record<string, unknown>> = [];
let flattenNode: ReturnType<typeof vi.fn>;

beforeAll(() => {
    registerActions();
});

/** A context-menu row by its visible label; menu items render a `.label` span. */
function menuItem(target: HTMLElement, label: string): HTMLButtonElement | null {
    const found = Array.from(target.querySelectorAll('button')).find(
        candidate => candidate.querySelector('.label')?.textContent === label,
    );
    return found instanceof HTMLButtonElement ? found : null;
}

/** Mount a layer row and open its context menu, as a right-click does. */
function openRowMenu(layer: ComponentProps<typeof LayerItem>['layer']) {
    const target = document.createElement('div');
    document.body.append(target);
    const instance = mount(LayerItem, { target, props: { layer, onupdate: vi.fn() } });
    mounted.push(instance as Record<string, unknown>);
    flushSync();

    target
        .querySelector('.layer-item')!
        .dispatchEvent(new MouseEvent('contextmenu', { bubbles: true }));
    flushSync();
    return target;
}

const smartObject = {
    type: 'void', id: 7, name: 'Smart Object', visible: true, editable: true,
    paintable: false, hasThumbnail: false, modifiers: [],
};

beforeEach(() => {
    flattenNode = vi.fn(async () => 8);
    const instance = new DarklyInstance();
    instance.engine = { api: { flattenNode } } as never;
    // The action refreshes the tree and repaints afterwards; neither has a
    // backing engine here, and a leaked frame callback throws asynchronously.
    instance.requestFrame = vi.fn();
    instance.refreshLayerTree = vi.fn();
    setActiveInstance(instance);
});

afterEach(() => {
    for (const instance of mounted.splice(0)) unmount(instance as never);
    document.body.replaceChildren();
    setActiveInstance(null);
});

describe('the layer row rasterize entry', () => {
    it('rasterizes the layer it was opened on', () => {
        const target = openRowMenu(smartObject);
        const entry = menuItem(target, 'Rasterize');

        expect(entry, 'a layer whose pixels are generated must offer Rasterize').not.toBeNull();
        entry!.click();

        expect(flattenNode).toHaveBeenCalledTimes(1);
        expect(flattenNode).toHaveBeenCalledWith({ node_id: 7 });
    });

    it('says Flatten instead when the layer owns its pixels and carries a mask', () => {
        const target = openRowMenu({
            type: 'raster', id: 3, name: 'Raster', visible: true, editable: true,
            paintable: true, hasThumbnail: false,
            modifiers: [{
                id: 42, kind: 'mask', name: 'Mask', visible: true, locked: false,
                linkedToHost: true, editable: true,
            }],
        });

        expect(menuItem(target, 'Rasterize')).toBeNull();
        menuItem(target, 'Flatten')!.click();
        expect(flattenNode).toHaveBeenCalledWith({ node_id: 3 });
    });

    it('offers neither for a plain raster that already is its own pixels', () => {
        const target = openRowMenu({
            type: 'raster', id: 4, name: 'Raster', visible: true, editable: true,
            paintable: true, hasThumbnail: false, modifiers: [],
        });

        expect(menuItem(target, 'Rasterize')).toBeNull();
        expect(menuItem(target, 'Flatten')).toBeNull();
    });
});
