<script lang="ts">
    /**
     * Test host for `BrushBarNode`. Supplies the `node-canvas` context its
     * real parent (`NodeCanvas`) provides; the node reads its rows from the
     * `brushGraph` store directly.
     */
    import { setContext } from 'svelte';
    import BrushBarNode from '../BrushBarNode.svelte';
    import type { NodeCanvasContext } from '../NodeCanvas.svelte';

    setContext<NodeCanvasContext>('node-canvas', {
        register: () => {},
        unregister: () => {},
        coords: {
            clientDeltaToGraph: (dx: number, dy: number) => ({ x: dx, y: dy }),
        } as NodeCanvasContext['coords'],
    });
</script>

<BrushBarNode />
