<script lang="ts">
    /**
     * Test host for `CurveEditor`. It reads pointer coordinates through the
     * `node-canvas` context because it normally lives inside the brush graph;
     * `FilterParamsEditor` supplies a trivial identity coord system for the
     * same reason (see its comment at :39). This harness does the same.
     */
    import { setContext } from 'svelte';
    import CurveEditor from '../../CurveEditor.svelte';
    import type { NodeCanvasContext } from '../../brush_builder/NodeCanvas.svelte';

    type Point = [number, number];
    let {
        points,
        onchange,
        oninput,
    }: {
        points: Point[];
        onchange: (p: Point[]) => void;
        oninput?: (p: Point[]) => void;
    } = $props();

    setContext<NodeCanvasContext>('node-canvas', {
        register: () => {},
        unregister: () => {},
        // Identity mapping: the test asserts in client pixels, and this panel
        // is neither zoomed nor panned, the same simplification
        // `FilterParamsEditor` makes at :39-43.
        coords: {
            clientToGraph: (x: number, y: number) => ({ x, y }),
            clientToElementLocal: (_el: Element, x: number, y: number) => ({ x, y }),
            clientDeltaToGraph: (dx: number, dy: number) => ({ x: dx, y: dy }),
            elementCenterInParent: () => ({ x: 0, y: 0 }),
        } as NodeCanvasContext['coords'],
    });
</script>

<CurveEditor {points} {onchange} {oninput} />
