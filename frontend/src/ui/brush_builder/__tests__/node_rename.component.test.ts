// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

// The real `app` is a Proxy over an active engine instance, which no
// component test stands up. A node drag only ever asks it to bracket an
// interaction, so a pair of no-ops is the whole surface needed here.
const { fakeApp } = vi.hoisted(() => ({
    fakeApp: { beginInteraction: () => {}, endInteraction: () => {}, engine: null },
}));
vi.mock('../../../state/app.svelte', () => ({ app: fakeApp, getActiveInstance: () => null }));

import NodeWidgetHarness from './NodeWidgetHarness.test.svelte';
import { brushGraph, type NodeInstance } from '../../../state/brush_graph.svelte';

const mounted: Array<Record<string, unknown>> = [];

/** A portless node: the header is the whole subject here, and no ports keeps
 *  `PortWidget` and the preview thumbnail out of the mount. */
function node(overrides: Partial<NodeInstance> = {}): NodeInstance {
    return { id: 'add_2', type_id: 'add', ports: [], ...overrides };
}

function render(n: NodeInstance) {
    brushGraph.graph = { nodes: { [n.id]: n }, connections: [] };
    brushGraph.nodePositions[n.id] = [0, 0];
    const target = document.createElement('div');
    document.body.append(target);
    mounted.push(mount(NodeWidgetHarness, { target, props: { nodeId: n.id } }) as Record<string, unknown>);
    flushSync();
    return target;
}

/** jsdom implements neither half of the pointer-capture API. */
function stubCapture(el: HTMLElement) {
    el.setPointerCapture = vi.fn();
    el.releasePointerCapture = vi.fn();
    return el.setPointerCapture as ReturnType<typeof vi.fn>;
}

const title = (t: HTMLElement) => t.querySelector<HTMLElement>('.node-title')!;
const nameInput = (t: HTMLElement) => t.querySelector<HTMLInputElement>('.node-title-edit');

beforeEach(() => {
    // The widget resolves its fallback label through the type registry.
    vi.spyOn(brushGraph, 'getNodeType').mockReturnValue({
        type_id: 'add',
        display_name: 'Add',
        category: 'math',
    } as ReturnType<typeof brushGraph.getNodeType>);
    vi.spyOn(brushGraph, 'setNodeName').mockResolvedValue(undefined);
});

afterEach(() => {
    for (const m of mounted.splice(0)) void unmount(m);
    document.body.innerHTML = '';
    brushGraph.graph = null;
    vi.restoreAllMocks();
});

describe('node rename', () => {
    it('an_unnamed_node_shows_its_type_name', () => {
        expect(title(render(node())).textContent).toBe('Add');
    });

    it('a_named_node_shows_the_name_and_keeps_the_type_in_the_tooltip', () => {
        const t = render(node({ name: 'Add pressure and tilt' }));

        expect(title(t).textContent).toBe('Add pressure and tilt');
        expect(title(t).title).toContain('Add');
    });

    // jsdom does not retarget compatibility mouse events under pointer
    // capture the way a browser does, so this passes either way; it guards the
    // affordance itself, not the capture bug. The invariant that keeps a click
    // from capturing at all is asserted below.
    it('double_clicking_the_title_opens_the_editor_even_after_a_press', () => {
        const t = render(node());

        title(t).dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerId: 1 }));
        title(t).dispatchEvent(new PointerEvent('pointerup', { bubbles: true, pointerId: 1 }));
        flushSync();
        title(t).dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
        flushSync();

        expect(nameInput(t)).not.toBeNull();
    });

    // Regression: the drag path used to `setPointerCapture` on pointerdown.
    // A browser retargets the compatibility mouse events at the capturing
    // card, so `dblclick` never reached the title and rename was unreachable
    // by its only affordance. Capture waits for the drag threshold, which
    // leaves a plain click uncaptured and its mouse events on their real
    // target. This is the assertion that fails without that fix.
    it('a_press_that_does_not_move_never_captures_the_pointer', () => {
        const t = render(node());
        const card = t.querySelector<HTMLElement>('.node-widget')!;
        const capture = stubCapture(card);

        card.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerId: 1, clientX: 50, clientY: 50 }));
        card.dispatchEvent(new PointerEvent('pointermove', { bubbles: true, pointerId: 1, clientX: 51, clientY: 50 }));
        flushSync();

        expect(capture).not.toHaveBeenCalled();
    });

    it('a_press_that_moves_past_the_threshold_starts_a_drag', () => {
        const t = render(node());
        const card = t.querySelector<HTMLElement>('.node-widget')!;
        const capture = stubCapture(card);

        card.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerId: 1, clientX: 50, clientY: 50 }));
        card.dispatchEvent(new PointerEvent('pointermove', { bubbles: true, pointerId: 1, clientX: 90, clientY: 70 }));
        flushSync();

        expect(capture).toHaveBeenCalled();
        expect(brushGraph.nodePositions['add_2']).toEqual([40, 20]);
    });

    it('committing_an_empty_name_restores_the_type_name', async () => {
        const t = render(node({ name: 'Add pressure and tilt' }));

        title(t).dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
        flushSync();
        const input = nameInput(t)!;
        input.value = '   ';
        input.dispatchEvent(new Event('input', { bubbles: true }));
        input.dispatchEvent(new FocusEvent('blur'));
        flushSync();

        expect(brushGraph.setNodeName).toHaveBeenCalledWith('add_2', '');
        expect(title(t).textContent).toBe('Add');
    });

    it('escape_abandons_the_edit_and_restores_the_prior_name', () => {
        const t = render(node({ name: 'Add pressure and tilt' }));

        title(t).dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
        flushSync();
        const input = nameInput(t)!;
        input.value = 'scrapped';
        input.dispatchEvent(new Event('input', { bubbles: true }));
        input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
        flushSync();

        expect(nameInput(t)).toBeNull();
        expect(title(t).textContent).toBe('Add pressure and tilt');
        expect(brushGraph.setNodeName).not.toHaveBeenCalled();
    });
});
