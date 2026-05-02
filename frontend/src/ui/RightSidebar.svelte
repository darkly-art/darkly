<script lang="ts">
    import PropertiesPanel from './properties/PropertiesPanel.svelte';
    import LayerPanel from './layers/LayerPanel.svelte';

    let sidebarEl: HTMLDivElement;
    let startX = 0;
    let startW = 0;
    let dragging = $state(false);

    function onResizeDown(e: PointerEvent) {
        e.preventDefault();
        startX = e.clientX;
        startW = sidebarEl.offsetWidth;
        dragging = true;
        (e.target as HTMLElement).setPointerCapture(e.pointerId);
    }

    function onResizeMove(e: PointerEvent) {
        if (!dragging) return;
        const dx = startX - e.clientX;
        sidebarEl.style.width = Math.max(180, Math.min(500, startW + dx)) + 'px';
    }

    function onResizeUp(e: PointerEvent) {
        dragging = false;
        (e.target as HTMLElement).releasePointerCapture(e.pointerId);
    }
</script>

<div class="sidebar" bind:this={sidebarEl}>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
        class="sidebar-resize"
        class:dragging
        onpointerdown={onResizeDown}
        onpointermove={onResizeMove}
        onpointerup={onResizeUp}
    ></div>
    <LayerPanel />
    <PropertiesPanel />
</div>

<style>
    .sidebar {
        width: 280px;
        min-width: 180px;
        max-width: 500px;
        background: var(--bg);
        display: flex;
        flex-direction: column;
        flex-shrink: 0;
        position: relative;
        overflow: hidden;
    }

    .sidebar-resize {
        position: absolute;
        left: 0;
        top: 0;
        bottom: 0;
        width: 4px;
        cursor: col-resize;
        z-index: 10;
    }

    .sidebar-resize:hover,
    .sidebar-resize.dragging {
        background: var(--accent);
    }
</style>
