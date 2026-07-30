// @vitest-environment jsdom
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import { DarklyInstance, setActiveInstance } from '../../../state/app.svelte';
import { registerActions } from '../../../actions';
import MaskChainHarness from './MaskChainHarness.svelte';
import MaskChainControl from '../MaskChainControl.svelte';
import LayerItem from '../LayerItem.svelte';
import LayerGroup from '../LayerGroup.svelte';

vi.mock('../thumbnails.svelte', () => ({
    THUMB_SIZE: 36,
    getNodeThumbnail: () => 'data:image/png;base64,AA==',
}));

const mounted: Array<Record<string, unknown>> = [];
let setMaskLinkedToHost: ReturnType<typeof vi.fn>;
let maskToSelection: ReturnType<typeof vi.fn>;

// The mask-menu "Mask to Selection" row dispatches the real action, so the
// registry must be populated for the dispatch to reach a handler.
beforeAll(() => {
    registerActions();
});

/** Find a context-menu row button by its visible label text. Menu items
 *  render a `.label` span (no aria-label), unlike the aria-labelled chain
 *  and thumbnail buttons. */
function menuItem(target: HTMLElement, label: string): HTMLButtonElement {
    const found = Array.from(target.querySelectorAll('button')).find(
        candidate => candidate.querySelector('.label')?.textContent === label,
    );
    if (!(found instanceof HTMLButtonElement)) throw new Error(`Missing menu item: ${label}`);
    return found;
}

function render(component: Parameters<typeof mount>[0], props: Record<string, unknown>) {
    const target = document.createElement('div');
    document.body.append(target);
    const instance = mount(component, { target, props });
    mounted.push(instance as Record<string, unknown>);
    flushSync();
    return { target, instance };
}

function button(target: HTMLElement, name: string): HTMLButtonElement {
    const found = Array.from(target.querySelectorAll('button')).find(
        candidate => candidate.getAttribute('aria-label') === name,
    );
    if (!(found instanceof HTMLButtonElement)) throw new Error(`Missing button: ${name}`);
    return found;
}

beforeEach(() => {
    setMaskLinkedToHost = vi.fn();
    maskToSelection = vi.fn();
    const instance = new DarklyInstance();
    instance.engine = { api: { setMaskLinkedToHost, maskToSelection } } as never;
    // The maskToSelection action requests a repaint; rAF + engine.render
    // neither exist nor are stubbed in the node test env, so a leaked frame
    // callback would throw asynchronously. Stub the frame request out.
    instance.requestFrame = vi.fn();
    setActiveInstance(instance);
});

afterEach(() => {
    for (const instance of mounted.splice(0)) unmount(instance as never);
    document.body.replaceChildren();
    setActiveInstance(null);
});

describe('MaskChainControl', () => {
    it.each([
        [true, 'Unlink mask from layer transforms'],
        [false, 'Link mask to layer transforms'],
    ])('renders projected linked state %s', (linked, label) => {
        const { target } = render(MaskChainHarness, { initialLinked: linked });
        expect(button(target, label).disabled).toBe(false);
    });

    it('gates edits when the mask relationship is not editable', () => {
        const { target } = render(MaskChainHarness, { editable: false });
        button(target, 'Unlink mask from layer transforms').click();
        expect(setMaskLinkedToHost).not.toHaveBeenCalled();
        expect(button(target, 'Unlink mask from layer transforms').disabled).toBe(true);
    });

    it('sends one desired transition and remains gated until projection refresh', () => {
        const { target, instance } = render(MaskChainHarness, {});
        const chain = button(target, 'Unlink mask from layer transforms');
        chain.click();
        chain.click();
        flushSync();

        expect(setMaskLinkedToHost).toHaveBeenCalledTimes(1);
        expect(setMaskLinkedToHost).toHaveBeenCalledWith({ id: 42, linked: false });
        expect(chain.disabled).toBe(true);

        flushSync(() => (instance as { project(linked: boolean): void }).project(false));
        expect(button(target, 'Link mask to layer transforms').disabled).toBe(false);
    });

    it('permits a second transition after an immediate authoritative refresh', () => {
        const { target } = render(MaskChainHarness, { refreshImmediately: true });
        button(target, 'Unlink mask from layer transforms').click();
        flushSync();
        button(target, 'Link mask to layer transforms').click();

        expect(setMaskLinkedToHost.mock.calls).toEqual([
            [{ id: 42, linked: false }],
            [{ id: 42, linked: true }],
        ]);
    });

    it('does not propagate chain activation to row selection', () => {
        const onselect = vi.fn();
        const { target } = render(MaskChainHarness, { onselect });
        button(target, 'Unlink mask from layer transforms').click();
        expect(onselect).not.toHaveBeenCalled();
    });

    it('renders the thumbnail as a keyboard-focusable selected-state button', () => {
        const onselect = vi.fn();
        const { target } = render(MaskChainHarness, { active: true, onselect });
        const thumbnail = button(target, 'Edit mask');

        expect(thumbnail.tabIndex).toBe(0);
        expect(thumbnail.getAttribute('aria-pressed')).toBe('true');
        thumbnail.focus();
        thumbnail.click();
        expect(document.activeElement).toBe(thumbnail);
        expect(onselect).toHaveBeenCalledTimes(1);
    });
});

describe('layer row mask integrations', () => {
    const modifier = {
        id: 42, kind: 'mask', name: 'Mask', visible: true, locked: false,
        linkedToHost: true, editable: true,
    };

    it.each([
        ['raster', LayerItem, {
            layer: {
                type: 'raster', id: 1, name: 'Raster', visible: true, editable: true,
                hasThumbnail: false, modifiers: [modifier],
            },
        }],
        ['group', LayerGroup, {
            group: {
                type: 'group', id: 2, name: 'Group', visible: true, editable: true,
                collapsed: true, passthrough: false, opacity: 1, blendMode: 'normal',
                children: [], modifiers: [modifier],
            },
        }],
    ])('renders the shared mask controls in the %s row', (_kind, component, props) => {
        const { target } = render(component as Parameters<typeof mount>[0], {
            ...props,
            onupdate: vi.fn(),
        });
        expect(button(target, 'Unlink mask from layer transforms')).toBeTruthy();
        expect(button(target, 'Edit mask')).toBeTruthy();
    });

    it.each([
        ['raster', LayerItem, {
            layer: {
                type: 'raster', id: 1, name: 'Raster', visible: true, editable: true,
                hasThumbnail: false, modifiers: [modifier],
            },
        }],
        ['group', LayerGroup, {
            group: {
                type: 'group', id: 2, name: 'Group', visible: true, editable: true,
                collapsed: true, passthrough: false, opacity: 1, blendMode: 'normal',
                children: [], modifiers: [modifier],
            },
        }],
    ])("the %s mask menu's Mask to Selection loads the mask as a selection", (_kind, component, props) => {
        const { target } = render(component as Parameters<typeof mount>[0], {
            ...props,
            onupdate: vi.fn(),
        });
        // Right-click the mask thumbnail to open its context menu.
        button(target, 'Edit mask').dispatchEvent(
            new MouseEvent('contextmenu', { bubbles: true }),
        );
        flushSync();
        menuItem(target, 'Mask to Selection').click();
        // The engine op takes the mask filter's id (42), not the host's.
        expect(maskToSelection).toHaveBeenCalledTimes(1);
        expect(maskToSelection).toHaveBeenCalledWith({ id: 42 });
    });
});
