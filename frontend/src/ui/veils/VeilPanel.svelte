<script lang="ts">
    import { app } from '../../state/app.svelte';
    import VeilItem from './VeilItem.svelte';

    function refresh() {
        app.refreshVeilList();
    }

    $effect(() => {
        if (app.handle) {
            refresh();
        }
    });

    function addPixelate() {
        if (app.handle) {
            app.handle.add_veil('pixelate', { scale: 2, soft: true });
            refresh();
        }
    }

    function addRainyGlass() {
        if (app.handle) {
            app.handle.add_veil('rainy_glass', { speed: 1.0, rain_amount: 0.7, direction: 0.0 });
            refresh();
        }
    }

    function onDragOver(e: DragEvent) {
        e.preventDefault();
    }

    function onDrop(e: DragEvent) {
        e.preventDefault();
    }
</script>

<div class="veil-panel">
    <div class="panel-header">
        <span>Veils</span>
    </div>

    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="veil-list" ondragover={onDragOver} ondrop={onDrop}>
        {#each app.veilList as veil (veil.index)}
            <VeilItem {veil} onupdate={refresh} />
        {/each}

        {#if app.veilList.length === 0}
            <div class="empty-message">No veils</div>
        {/if}
    </div>

    <div class="panel-actions">
        <button class="action-btn" onclick={addPixelate} title="Add pixelate veil">+ pixelate</button>
        <button class="action-btn" onclick={addRainyGlass} title="Add rainy glass veil">+ rainy glass</button>
    </div>
</div>

<style>
    .veil-panel {
        display: flex;
        flex-direction: column;
        max-height: 200px;
        border-bottom: 1px solid #333;
    }

    .panel-header {
        padding: 8px 12px;
        font-size: 11px;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 0.5px;
        color: #888;
        border-bottom: 1px solid #333;
    }

    .veil-list {
        flex: 1;
        overflow-y: auto;
        padding: 4px 0;
    }

    .empty-message {
        padding: 8px;
        text-align: center;
        color: #555;
        font-size: 12px;
    }

    .panel-actions {
        display: flex;
        gap: 4px;
        padding: 6px 8px;
        border-top: 1px solid #333;
    }

    .action-btn {
        flex: 1;
        background: #2a2a2a;
        border: 1px solid #444;
        border-radius: 4px;
        color: #ccc;
        cursor: pointer;
        padding: 3px;
        font-size: 11px;
    }

    .action-btn:hover {
        background: #333;
    }
</style>
