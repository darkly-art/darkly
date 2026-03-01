<script lang="ts">
    import { toolRegistry } from '../tools/registry';
    import { app } from '../state/app.svelte';
    import { canvasToScreen, screenToCanvas } from './coordinates';
    import type { OverlayHandle } from './overlay';

    interface Props {
        canvasEl: HTMLCanvasElement;
    }
    let { canvasEl }: Props = $props();

    // Reactively read the active tool's overlay
    let overlay = $derived.by(() => {
        const tool = toolRegistry.get(app.activeToolId);
        if (!tool?.getOverlay) return null;
        // Read view state to establish reactive dependency
        void app.panX; void app.panY; void app.zoom; void app.rotation;
        return tool.getOverlay();
    });

    // --- Handle dragging ---
    let dragging: OverlayHandle | null = $state(null);

    function onHandlePointerDown(e: PointerEvent, handle: OverlayHandle) {
        if (!handle.onDrag) return;
        e.stopPropagation();
        e.preventDefault();
        dragging = handle;
    }

    function onPointerMove(e: PointerEvent) {
        if (!dragging) return;
        const pos = screenToCanvas(e.clientX, e.clientY, canvasEl);
        dragging.onDrag!(pos.x, pos.y);
    }

    function onPointerUp() {
        if (!dragging) return;
        dragging.onDragEnd?.();
        dragging = null;
    }

    function toScreen(cx: number, cy: number) {
        return canvasToScreen(cx, cy, canvasEl);
    }
</script>

{#if overlay}
<!-- svelte-ignore a11y_no_static_element_interactions -->
<svg
    class="tool-overlay"
    aria-hidden="true"
    style:pointer-events={dragging ? 'all' : 'none'}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
>
    {#each overlay.lines ?? [] as line}
        {@const p1 = toScreen(line.x1, line.y1)}
        {@const p2 = toScreen(line.x2, line.y2)}
        <line
            x1={p1.x} y1={p1.y}
            x2={p2.x} y2={p2.y}
            stroke={line.stroke ?? 'white'}
            stroke-width={line.strokeWidth ?? 1}
            stroke-dasharray={line.dashArray ?? '6 3'}
        />
    {/each}

    {#each overlay.handles ?? [] as handle (handle.id)}
        {@const p = toScreen(handle.x, handle.y)}
        <circle
            cx={p.x} cy={p.y}
            r={handle.radius ?? 6}
            fill={handle.fill ?? 'white'}
            stroke={handle.stroke ?? '#333'}
            stroke-width="1.5"
            style:cursor={handle.cursor ?? (handle.onDrag ? 'grab' : 'default')}
            onpointerdown={(e) => onHandlePointerDown(e, handle)}
        />
    {/each}
</svg>
{/if}

<style>
    .tool-overlay {
        position: absolute;
        inset: 0;
        width: 100%;
        height: 100%;
        overflow: visible;
        z-index: 1;
    }

    .tool-overlay line {
        pointer-events: none;
    }

    .tool-overlay circle {
        pointer-events: all;
    }
</style>
