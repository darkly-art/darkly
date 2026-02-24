<script lang="ts">
    import { app } from '../../state/app.svelte';

    let { layer, onupdate }: {
        layer: { type: string; id: number; name: string; visible: boolean; opacity: number; blendMode: number };
        onupdate: () => void;
    } = $props();

    let isActive = $derived(app.activeLayerId === layer.id);

    function toggleVisibility() {
        if (app.handle) {
            app.handle.set_layer_visible(BigInt(layer.id), !layer.visible);
            onupdate();
        }
    }

    function setActive() {
        app.activeLayerId = layer.id;
    }

    function onOpacityChange(e: Event) {
        const value = parseFloat((e.target as HTMLInputElement).value);
        if (app.handle) {
            app.handle.set_opacity(BigInt(layer.id), value);
            onupdate();
        }
    }

    function onBlendModeChange(e: Event) {
        const value = parseInt((e.target as HTMLSelectElement).value);
        if (app.handle) {
            app.handle.set_blend_mode(BigInt(layer.id), value);
            onupdate();
        }
    }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div
    class="layer-item"
    class:active={isActive}
    onclick={setActive}
    onkeydown={(e: KeyboardEvent) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); setActive(); }}}
    role="button"
    tabindex="0"
    draggable="true"
    ondragstart={(e: DragEvent) => {
        e.dataTransfer?.setData('text/plain', String(layer.id));
    }}
>
    <button
        class="vis-btn"
        class:hidden={!layer.visible}
        onclick={(e: MouseEvent) => { e.stopPropagation(); toggleVisibility(); }}
        title="Toggle visibility"
    >
        {layer.visible ? '\u{1F441}' : '\u{2014}'}
    </button>

    <span class="layer-name">{layer.name}</span>

    {#if layer.type === 'raster'}
        <input
            type="range"
            class="opacity-slider"
            min="0" max="1" step="0.01"
            value={layer.opacity}
            oninput={onOpacityChange}
            onclick={(e: MouseEvent) => e.stopPropagation()}
            title="Opacity: {Math.round(layer.opacity * 100)}%"
        />
    {/if}
</div>

<style>
    .layer-item {
        display: flex;
        align-items: center;
        gap: 4px;
        padding: 4px 8px;
        cursor: pointer;
        border-left: 3px solid transparent;
        min-height: 28px;
    }

    .layer-item:hover {
        background: #2a2a2a;
    }

    .layer-item.active {
        background: #2a2a3a;
        border-left-color: #6a6aff;
    }

    .vis-btn {
        background: none;
        border: none;
        color: #888;
        cursor: pointer;
        padding: 0;
        font-size: 12px;
        width: 18px;
        text-align: center;
    }
    .vis-btn.hidden { color: #444; }

    .layer-name {
        flex: 1;
        font-size: 12px;
        color: #ccc;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .opacity-slider {
        width: 50px;
        height: 12px;
        accent-color: #6a6aff;
    }
</style>
