// @vitest-environment jsdom
//
// Two live bugs in the layer row's menus, fixed ahead of the
// LayerItem/LayerGroup merge so that merge stays a pure refactor.
//
// 1. "Apply mask" is offered enabled on rows whose pixels are generated. The
//    engine's `apply_mask` is raster only (`crates/darkly/src/engine/filters/
//    mask.rs:231-239`) and `paintable` is bit-for-bit that same predicate, so
//    clicking the entry on a void / filter / vector row does nothing at all:
//    no engine call, no error, nothing in the console.
// 2. "Merge Down" is offered on a bottom-most root layer whose only row below
//    is the viewport divider, which is not a layer and cannot be merged into.
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import { DarklyInstance, setActiveInstance } from '../../../state/app.svelte';
import { registerActions } from '../../../actions';
import LayerRow from '../LayerRow.svelte';
import { maskModifier, rasterNode, voidNode } from './rowFixtures';

vi.mock('../thumbnails.svelte', () => ({
    THUMB_SIZE: 36,
    getNodeThumbnail: () => 'data:image/png;base64,AA==',
}));

const mounted: Array<Record<string, unknown>> = [];

beforeAll(() => {
    registerActions();
});

/** A context-menu row by its visible label; menu items render a `.label` span. */
function menuItem(target: HTMLElement, label: string): HTMLButtonElement | null {
    const found = Array.from(target.querySelectorAll('button')).find(
        (candidate) => candidate.querySelector('.label')?.textContent === label,
    );
    return found instanceof HTMLButtonElement ? found : null;
}

function mountRow(layer: Record<string, unknown>) {
    const target = document.createElement('div');
    document.body.append(target);
    mounted.push(
        mount(LayerRow, { target, props: { node: layer, onupdate: vi.fn() } as never }) as Record<string, unknown>,
    );
    flushSync();
    return target;
}

function rightClick(el: Element) {
    el.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true }));
    flushSync();
}

const mask = { ...maskModifier, id: 99 };

function instanceWithTree(tree: unknown[] = []) {
    const inst = new DarklyInstance();
    inst.engine = { api: {} } as never;
    inst.requestFrame = vi.fn();
    inst.layerTree = tree as never;
    setActiveInstance(inst);
    return inst;
}

afterEach(() => {
    for (const instance of mounted.splice(0)) unmount(instance as never);
    document.body.replaceChildren();
    setActiveInstance(null);
});

describe('Apply mask', () => {
    beforeEach(() => instanceWithTree());

    it('is disabled on a void row, because apply_mask is raster only', () => {
        const target = mountRow(voidNode({ id: 7, modifiers: [mask] }) as never);
        rightClick(target.querySelector('[aria-label="Edit mask"]')!);
        const entry = menuItem(target, 'Apply mask');
        expect(entry).not.toBeNull();
        expect(entry!.disabled).toBe(true);
    });

    it('stays enabled on a raster row', () => {
        const target = mountRow(rasterNode({ id: 3, modifiers: [mask] }) as never);
        rightClick(target.querySelector('[aria-label="Edit mask"]')!);
        expect(menuItem(target, 'Apply mask')!.disabled).toBe(false);
    });
});

describe('Merge Down', () => {
    it('is disabled when the only row below is the viewport divider', () => {
        const layer = rasterNode({ id: 3 });
        instanceWithTree([layer, { type: 'divider', id: -1 }]);
        const target = mountRow(layer);
        rightClick(target.querySelector('.layer-row')!);
        expect(menuItem(target, 'Merge Down')!.disabled).toBe(true);
    });

    it('stays enabled when a real layer sits below', () => {
        const layer = rasterNode({ id: 3 });
        instanceWithTree([layer, rasterNode({ id: 4, name: 'Below' })]);
        const target = mountRow(layer);
        rightClick(target.querySelector('.layer-row')!);
        expect(menuItem(target, 'Merge Down')!.disabled).toBe(false);
    });
});
