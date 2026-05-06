<script lang="ts">
    import { app } from '../../state/app.svelte';

    let { node }: {
        node: { id: number; opacity: number; blendMode: number };
    } = $props();

    const BLEND_MODE_GROUPS: { label: string; modes: { value: number; label: string }[] }[] = [
        { label: 'Normal', modes: [
            { value: 0, label: 'Normal' },
        ]},
        { label: 'Darken', modes: [
            { value: 1, label: 'Darken' },
            { value: 2, label: 'Multiply' },
            { value: 3, label: 'Color Burn' },
        ]},
        { label: 'Lighten', modes: [
            { value: 4, label: 'Lighten' },
            { value: 5, label: 'Screen' },
            { value: 6, label: 'Color Dodge' },
            { value: 7, label: 'Linear Dodge (Add)' },
        ]},
        { label: 'Contrast', modes: [
            { value: 8, label: 'Overlay' },
            { value: 9, label: 'Soft Light' },
            { value: 10, label: 'Hard Light' },
        ]},
        { label: 'Inversion', modes: [
            { value: 11, label: 'Difference' },
        ]},
        { label: 'Component', modes: [
            { value: 12, label: 'Hue' },
            { value: 13, label: 'Saturation' },
            { value: 14, label: 'Color' },
            { value: 15, label: 'Luminosity' },
        ]},
    ];

    function onOpacityInput(e: Event) {
        const value = parseFloat((e.target as HTMLInputElement).value);
        app.handle?.set_opacity(node.id, value);
        app.refreshLayerTree();
        app.requestFrame();
    }

    function onBlendModeChange(e: Event) {
        const value = parseInt((e.target as HTMLSelectElement).value, 10);
        app.handle?.set_blend_mode(node.id, value);
        app.refreshLayerTree();
        app.requestFrame();
    }
</script>

<div class="row">
    <span class="label">Opacity</span>
    <input
        type="range"
        class="slider"
        min="0" max="1" step="0.01"
        value={node.opacity}
        oninput={onOpacityInput}
    />
    <span class="value">{Math.round((node.opacity ?? 1) * 100)}%</span>
</div>

<div class="row">
    <span class="label">Blend</span>
    <select class="select" value={node.blendMode ?? 0} onchange={onBlendModeChange}>
        {#each BLEND_MODE_GROUPS as group (group.label)}
            <optgroup label={group.label}>
                {#each group.modes as bm (bm.value)}
                    <option value={bm.value}>{bm.label}</option>
                {/each}
            </optgroup>
        {/each}
    </select>
</div>

<style>
    .row {
        display: flex;
        align-items: center;
        gap: 8px;
        min-height: 22px;
    }

    .label {
        font-size: 11px;
        color: var(--text-muted);
        min-width: 56px;
    }

    .slider {
        flex: 1;
        height: 4px;
        min-width: 0;
    }

    .value {
        font-size: 11px;
        color: var(--text-muted);
        min-width: 36px;
        text-align: right;
        font-variant-numeric: tabular-nums;
    }

    .select {
        flex: 1;
        background: var(--bg-hover);
        color: var(--text);
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-sm);
        padding: 3px 6px;
        font-size: 12px;
        outline: none;
        min-width: 0;
    }

    .select:focus {
        border-color: var(--accent);
    }
</style>
