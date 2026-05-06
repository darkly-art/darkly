<script lang="ts">
    import { brushGraph } from '../state/brush_graph.svelte';
    import BrushBuilder from './brush_builder/BrushBuilder.svelte';

    let builderHeight = $state(33); // vh units

    function handleResizeStart(e: PointerEvent) {
        e.preventDefault();
        const el = e.currentTarget as HTMLElement;
        el.setPointerCapture(e.pointerId);
        const startY = e.clientY;
        const startHeight = builderHeight;
        const vh = window.innerHeight / 100;

        const onMove = (ev: PointerEvent) => {
            const dy = startY - ev.clientY; // dragging up = increase height
            builderHeight = Math.min(80, Math.max(15, startHeight + dy / vh));
        };
        const onUp = () => {
            el.removeEventListener('pointermove', onMove);
            el.removeEventListener('pointerup', onUp);
        };
        el.addEventListener('pointermove', onMove);
        el.addEventListener('pointerup', onUp);
    }
</script>

{#if brushGraph.isOpen}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="resize-handle" onpointerdown={handleResizeStart}></div>
    <div class="builder-panel" style="height: {builderHeight}vh">
        <BrushBuilder />
    </div>
{/if}

<style>
    .resize-handle {
        height: 5px;
        cursor: ns-resize;
        background: transparent;
        flex-shrink: 0;
        transition: background 0.1s;
    }
    .resize-handle:hover,
    .resize-handle:active {
        background: var(--accent);
    }

    .builder-panel {
        min-height: 100px;
        border-bottom: 1px solid var(--bg-hover);
    }
</style>
