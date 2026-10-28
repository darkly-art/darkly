// @vitest-environment jsdom
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

// The real `app` is a Proxy over an active engine instance, which no
// component test stands up. Hovering never reaches it; a drag only brackets
// an interaction.
const { fakeApp } = vi.hoisted(() => ({
    fakeApp: { beginInteraction: () => {}, endInteraction: () => {}, engine: null },
}));
vi.mock('../../../state/app.svelte', () => ({ app: fakeApp, getActiveInstance: () => null }));

import BrushBarNodeHarness from './BrushBarNodeHarness.test.svelte';
import NodeWidgetHarness from './NodeWidgetHarness.test.svelte';
import {
    brushGraph,
    type ExposedPortInfo,
    type NodeInstance,
    type PortDef,
} from '../../../state/brush_graph.svelte';

const mounted: Array<Record<string, unknown>> = [];

const ENTRY: ExposedPortInfo = {
    key: 'multiply_2.a',
    nodeId: 'multiply_2',
    portName: 'a',
    label: 'Twirl',
    portLabel: 'A',
    icon: '',
    description: '',
    nodeDisplayName: 'Twirl amount',
    data: {
        kind: 'scalar',
        value: 0.5,
        min: 0,
        max: 1,
        default: 1,
        unitType: 'Normalized',
        invert: false,
        unitOverride: null,
        inheritedUnit: 'Normalized',
        travel: { exponent: 1, reversed: false },
    },
};

const PORT: PortDef = {
    name: 'a',
    dir: 'Input',
    wire_type: 'Scalar',
    min: 0,
    max: 1,
    travel_exponent: 1,
    step: 0,
    value: 0.5,
    wirable: true,
    source: false,
    preview_image: false,
    exposable: true,
    description: '',
    unit_type: 'Normalized',
    icon: '',
    label: '',
    exposed: true,
};

const NODE: NodeInstance = { id: 'multiply_2', type_id: 'multiply', ports: [PORT] };

function newTarget() {
    const target = document.createElement('div');
    document.body.append(target);
    return target;
}

function renderBrushBar() {
    brushGraph.exposedPorts = [ENTRY];
    const target = newTarget();
    const m = mount(BrushBarNodeHarness, { target }) as Record<string, unknown>;
    mounted.push(m);
    flushSync();
    return { target, m };
}

function renderNode(exposed: boolean) {
    brushGraph.graph = { nodes: { [NODE.id]: NODE }, connections: [] };
    brushGraph.nodePositions[NODE.id] = [0, 0];
    brushGraph.exposedPorts = exposed ? [ENTRY] : [];
    const target = newTarget();
    mounted.push(mount(NodeWidgetHarness, { target, props: { nodeId: NODE.id } }) as Record<string, unknown>);
    flushSync();
    return target;
}

/** Svelte attaches `pointerenter` / `pointerleave` directly, so a
 *  non-bubbling event on the element reaches the handler. */
function hover(el: Element, type: 'pointerenter' | 'pointerleave') {
    el.dispatchEvent(new PointerEvent(type));
    flushSync();
}

const LIT = { node: 'multiply_2', port: 'a' };

const row = (t: HTMLElement) => t.querySelector<HTMLElement>('.entry-row')!;
const portRow = (t: HTMLElement) => t.querySelector<HTMLElement>('.port-row')!;

beforeAll(() => {
    // jsdom implements neither; `Modal` bridges `open` through them.
    HTMLDialogElement.prototype.showModal = function () {
        this.open = true;
    };
    HTMLDialogElement.prototype.close = function () {
        this.open = false;
    };
});

beforeEach(() => {
    vi.spyOn(brushGraph, 'getNodeType').mockReturnValue({
        type_id: 'multiply',
        display_name: 'Multiply',
        category: 'math',
        ports: [],
    } as unknown as ReturnType<typeof brushGraph.getNodeType>);
});

afterEach(() => {
    for (const m of mounted.splice(0)) void unmount(m);
    document.body.innerHTML = '';
    brushGraph.graph = null;
    brushGraph.exposedPorts = [];
    brushGraph.highlightedPort = null;
    vi.restoreAllMocks();
});

describe('brush bar to graph', () => {
    it('hovering_a_row_highlights_its_port', () => {
        const { target } = renderBrushBar();
        hover(row(target), 'pointerenter');
        expect(brushGraph.highlightedPort).toEqual(LIT);
    });

    it('leaving_the_row_clears_the_highlight', () => {
        const { target } = renderBrushBar();
        hover(row(target), 'pointerenter');
        expect(brushGraph.highlightedPort).toEqual(LIT);
        hover(row(target), 'pointerleave');
        expect(brushGraph.highlightedPort).toBeNull();
    });

    it('an_open_editor_keeps_the_highlight_until_it_closes', () => {
        const { target } = renderBrushBar();
        hover(row(target), 'pointerenter');
        target.querySelector<HTMLElement>('.row-edit')!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
        flushSync();
        hover(row(target), 'pointerleave');
        expect(brushGraph.highlightedPort).toEqual(LIT);

        const dialog = target.querySelector('dialog')!;
        expect(dialog.textContent).toContain('A on Twirl amount');

        dialog.dispatchEvent(new Event('close'));
        flushSync();
        expect(brushGraph.highlightedPort).toBeNull();
    });

    it('a_finished_drag_clears_the_highlight', () => {
        const { target } = renderBrushBar();
        hover(row(target), 'pointerenter');
        expect(brushGraph.highlightedPort).toEqual(LIT);
        // jsdom has no `DragEvent`; the handler guards `dataTransfer`.
        row(target).dispatchEvent(new Event('dragstart', { bubbles: true }));
        target.querySelector('.rows')!.dispatchEvent(new Event('dragend', { bubbles: true }));
        flushSync();
        expect(brushGraph.highlightedPort).toBeNull();
    });

    it('unmounting_the_brush_bar_clears_the_highlight', () => {
        const { target, m } = renderBrushBar();
        hover(row(target), 'pointerenter');
        expect(brushGraph.highlightedPort).toEqual(LIT);
        void unmount(m);
        mounted.splice(mounted.indexOf(m), 1);
        flushSync();
        expect(brushGraph.highlightedPort).toBeNull();
    });

    it('a_highlighted_port_lights_its_row', () => {
        const { target } = renderBrushBar();
        brushGraph.highlightedPort = LIT;
        flushSync();
        expect(target.querySelector('.entry-row.highlighted')).not.toBeNull();

        brushGraph.highlightedPort = null;
        flushSync();
        expect(target.querySelector('.entry-row.highlighted')).toBeNull();
    });
});

describe('graph to brush bar', () => {
    it('hovering_an_exposed_port_sets_the_highlight_and_leaving_clears_it', () => {
        const t = renderNode(true);
        hover(portRow(t), 'pointerenter');
        expect(brushGraph.highlightedPort).toEqual(LIT);

        hover(portRow(t), 'pointerleave');
        expect(brushGraph.highlightedPort).toBeNull();
    });

    it('hovering_an_unexposed_port_sets_nothing', () => {
        const t = renderNode(false);
        hover(portRow(t), 'pointerenter');
        expect(brushGraph.highlightedPort).toBeNull();
    });

    it('unexposing_a_hovered_port_still_clears_on_leave', () => {
        const t = renderNode(true);
        hover(portRow(t), 'pointerenter');
        expect(brushGraph.highlightedPort).toEqual(LIT);
        brushGraph.exposedPorts = [];
        flushSync();
        hover(portRow(t), 'pointerleave');
        expect(brushGraph.highlightedPort).toBeNull();
    });

    it('the_highlighted_port_row_and_its_card_carry_the_class', () => {
        const t = renderNode(true);
        brushGraph.highlightedPort = LIT;
        flushSync();
        expect(t.querySelector('.port-row.highlighted')).not.toBeNull();
        expect(t.querySelector('.node-widget.highlighted')).not.toBeNull();

        brushGraph.highlightedPort = null;
        flushSync();
        expect(t.querySelector('.port-row.highlighted')).toBeNull();
        expect(t.querySelector('.node-widget.highlighted')).toBeNull();
    });
});
