// @vitest-environment jsdom
//
// The layer row's Convert to Smart Object entry, end to end: right-click →
// click → engine op.
//
// A predicate test cannot see whether the entry actually reaches the menu, and
// that is exactly where this feature failed before: the engine op and its
// predicate were implemented and green while no entry existed in any menu at
// all. So this mounts the real row, opens the real menu, and clicks the real
// button.
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
let convertLayerToSmartObject: ReturnType<typeof vi.fn>;

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

const convertible = {
    type: 'raster', id: 3, name: 'Raster', visible: true, editable: true,
    paintable: true, hasThumbnail: true, canBecomeSmartObject: true, modifiers: [],
};

beforeEach(() => {
    convertLayerToSmartObject = vi.fn(async () => 9);
    const instance = new DarklyInstance();
    instance.engine = { api: { convertLayerToSmartObject } } as never;
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

describe('the layer row convert-to-smart-object entry', () => {
    it('is present on a convertible layer', () => {
        const target = openRowMenu(convertible);
        expect(
            menuItem(target, 'Convert to Smart Object'),
            'a convertible layer must offer the conversion',
        ).not.toBeNull();
    });

    it('converts the layer it was opened on', () => {
        const target = openRowMenu(convertible);
        menuItem(target, 'Convert to Smart Object')!.click();

        expect(convertLayerToSmartObject).toHaveBeenCalledTimes(1);
        expect(convertLayerToSmartObject).toHaveBeenCalledWith({ node_id: 3 });
    });

    it('is absent when the engine says the layer cannot become one', () => {
        const target = openRowMenu({ ...convertible, canBecomeSmartObject: false });
        expect(menuItem(target, 'Convert to Smart Object')).toBeNull();
    });

    it('is absent on a smart object, which already holds its source', () => {
        const target = openRowMenu({
            type: 'void', id: 7, name: 'Smart Object', visible: true, editable: true,
            paintable: false, hasThumbnail: true, canBecomeSmartObject: false, modifiers: [],
        });
        expect(menuItem(target, 'Convert to Smart Object')).toBeNull();
    });
});
