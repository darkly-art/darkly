// @vitest-environment jsdom
//
// The container row, through the merged `LayerRow`.
//
// Written first against the old `LayerGroup` and re-pointed here, which is what
// makes it a characterization test rather than a description of the new code:
// every assertion passed before the merge and passes after it.
//
// This was the coverage gap the merge was riskiest without. Of the three layer
// component test files that existed before it, two never mounted `LayerGroup`
// at all and the third only exercised the mask sub-row. The trap it guards is
// specific: a group reports `paintable: false`, so a naive merge relabels every
// group's "Flatten" entry to "Rasterize" and nothing notices.
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import { DarklyInstance, setActiveInstance } from '../../../state/app.svelte';
import { registerActions } from '../../../actions';
import LayerRow from '../LayerRow.svelte';

vi.mock('../thumbnails.svelte', () => ({
    THUMB_SIZE: 36,
    getNodeThumbnail: () => 'data:image/png;base64,AA==',
}));

const mounted: Array<Record<string, unknown>> = [];
let setGroupCollapsed: ReturnType<typeof vi.fn>;
let setLayerName: ReturnType<typeof vi.fn>;

beforeAll(() => {
    registerActions();
});

function menuItem(target: HTMLElement, label: string): HTMLButtonElement | null {
    const found = Array.from(target.querySelectorAll('button')).find(
        (candidate) => candidate.querySelector('.label')?.textContent === label,
    );
    return found instanceof HTMLButtonElement ? found : null;
}

function child(id: number, name = `Child ${id}`) {
    return {
        type: 'raster', id, name, visible: true, locked: false, editable: true,
        paintable: true, canHaveMask: true, canRename: true, hasThumbnail: true,
        canBecomeSmartObject: false, icon: 'fa6-solid:image', kindName: 'Raster Layer',
        opacity: 1, blendMode: 'normal', modifiers: [],
    };
}

function group(overrides: Record<string, unknown> = {}) {
    return {
        type: 'group', id: 2, name: 'Group', visible: true, locked: false, editable: true,
        paintable: false, canHaveMask: true, canRename: true, hasThumbnail: false,
        canBecomeSmartObject: false, icon: 'fa6-solid:folder', kindName: 'Group',
        collapsed: false, passthrough: false, opacity: 1, blendMode: 'normal',
        modifiers: [], children: [child(3)],
        ...overrides,
    };
}

function render(g: Record<string, unknown>) {
    const target = document.createElement('div');
    document.body.append(target);
    mounted.push(
        mount(LayerRow, { target, props: { node: g, onupdate: vi.fn() } as never }) as Record<string, unknown>,
    );
    flushSync();
    return target;
}

function row(target: HTMLElement): HTMLElement {
    return target.querySelector('.layer-row') as HTMLElement;
}

function rightClick(el: Element) {
    el.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true }));
    flushSync();
}

beforeEach(() => {
    setGroupCollapsed = vi.fn();
    setLayerName = vi.fn();
    const inst = new DarklyInstance();
    inst.engine = { api: { setGroupCollapsed, setLayerName } } as never;
    inst.requestFrame = vi.fn();
    inst.layerTree = [group()] as never;
    setActiveInstance(inst);
});

afterEach(() => {
    for (const instance of mounted.splice(0)) unmount(instance as never);
    document.body.replaceChildren();
    setActiveInstance(null);
});

describe('group row menu', () => {
    it('offers Flatten, never Rasterize, even though a group is not paintable', () => {
        const target = render(group());
        rightClick(row(target));
        expect(menuItem(target, 'Flatten')).not.toBeNull();
        expect(menuItem(target, 'Rasterize')).toBeNull();
    });

    it('names delete and duplicate after the group for a single selection', () => {
        const target = render(group());
        rightClick(row(target));
        expect(menuItem(target, 'Delete Group')).not.toBeNull();
        expect(menuItem(target, 'Duplicate Group')).not.toBeNull();
    });
});

describe('group row collapse', () => {
    it('toggles the flag through the engine, negated', () => {
        const target = render(group({ collapsed: false }));
        (target.querySelector('.collapse-btn') as HTMLButtonElement).click();
        flushSync();
        expect(setGroupCollapsed).toHaveBeenCalledWith({ id: 2, collapsed: true });
    });

    it('renders a row per child when expanded and none when collapsed', () => {
        const expanded = render(group({ collapsed: false, children: [child(3), child(4)] }));
        // The container's own row plus one per child.
        expect(expanded.querySelectorAll('.layer-row').length).toBe(3);

        const collapsed = render(group({ collapsed: true, children: [child(3), child(4)] }));
        expect(collapsed.querySelectorAll('.layer-row').length).toBe(1);
    });

    it('indents each child one level, at 8 + depth * 16 px', () => {
        const target = render(group({ collapsed: false, children: [child(3)] }));
        expect(row(target).style.paddingLeft).toBe('8px');
        const childRow = target.querySelectorAll('.layer-row')[1] as HTMLElement;
        expect(childRow.style.paddingLeft).toBe('24px');
    });
});

describe('group row rename', () => {
    it('double-click starts a rename and blur commits the typed value', () => {
        const target = render(group());
        row(target).dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
        flushSync();

        const input = target.querySelector('.name-input') as HTMLInputElement;
        expect(input).not.toBeNull();
        input.value = 'Renamed';
        input.dispatchEvent(new FocusEvent('blur', { bubbles: true }));
        flushSync();

        expect(setLayerName).toHaveBeenCalledWith({ id: 2, name: 'Renamed' });
    });
});
