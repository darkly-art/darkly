<script lang="ts">
    /**
     * Test host for `NodeWidget`. Supplies the port-registration context its
     * real parent (`NodeCanvas`) provides, and feeds the widget its node out
     * of the `brushGraph` store the same way the canvas does, so the store's
     * local-echo setters reach the rendered node.
     */
    import { setContext } from 'svelte';
    import NodeWidget from '../NodeWidget.svelte';
    import type { NodeCanvasContext } from '../NodeCanvas.svelte';
    import { brushGraph } from '../../../state/brush_graph.svelte';

    let { nodeId }: { nodeId: string } = $props();

    let node = $derived(brushGraph.nodeList.find(n => n.id === nodeId)!);

    setContext<NodeCanvasContext>('node-canvas', {
        register: () => {},
        unregister: () => {},
        // Identity mapping: the tests that drag assert in client pixels.
        coords: {
            clientDeltaToGraph: (dx: number, dy: number) => ({ x: dx, y: dy }),
        } as NodeCanvasContext['coords'],
    });
</script>

<NodeWidget {node} />
