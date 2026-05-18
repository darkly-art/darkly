<script lang="ts">
    import { app } from '../../state/app.svelte';

    let { node }: {
        node: { id: number; opacity: number; blendMode: string; editable?: boolean };
    } = $props();

    // Mirror the engine's `is_node_editable` predicate — when false, both
    // setters no-op, so the controls would be drag-but-nothing-happens.
    // Disabling here keeps the UI honest about what's settable.
    let editable = $derived(node.editable !== false);

    // Blend modes come from the Rust BlendModeRegistry — the dropdown
    // (and its category-based <optgroup>s) is built entirely from that table.
    interface BlendModeType { type: string; displayName: string; category: string; }
    let blendModeTypes = $state<BlendModeType[]>([]);
    $effect(() => {
        if (!app.handle) return;
        try {
            blendModeTypes = JSON.parse(app.handle.blend_mode_types()) as BlendModeType[];
        } catch {
            blendModeTypes = [];
        }
    });

    interface BlendModeGroup { label: string; modes: BlendModeType[]; }
    let blendModeGroups = $derived((() => {
        const groups: BlendModeGroup[] = [];
        let current: BlendModeGroup | null = null;
        for (const bm of blendModeTypes) {
            if (!current || current.label !== bm.category) {
                current = { label: bm.category, modes: [] };
                groups.push(current);
            }
            current.modes.push(bm);
        }
        return groups;
    })());

    function onOpacityInput(e: Event) {
        const value = parseFloat((e.target as HTMLInputElement).value);
        app.handle?.set_opacity(node.id, value);
        app.refreshLayerTree();
        app.requestFrame();
    }

    function onBlendModeChange(e: Event) {
        const value = (e.target as HTMLSelectElement).value;
        app.handle?.set_blend_mode(node.id, value);
        app.refreshLayerTree();
        app.requestFrame();
    }
</script>

<div class="row" class:disabled={!editable}>
    <span class="label">Blend</span>
    <select
        class="select"
        value={node.blendMode ?? 'normal'}
        onchange={onBlendModeChange}
        disabled={!editable}
    >
        {#each blendModeGroups as group (group.label)}
            <optgroup label={group.label}>
                {#each group.modes as bm (bm.type)}
                    <option value={bm.type}>{bm.displayName}</option>
                {/each}
            </optgroup>
        {/each}
    </select>
</div>

<div class="row" class:disabled={!editable}>
    <span class="label">Opacity</span>
    <input
        type="range"
        class="slider"
        min="0" max="1" step="0.01"
        value={node.opacity}
        oninput={onOpacityInput}
        disabled={!editable}
    />
    <span class="value">{Math.round((node.opacity ?? 1) * 100)}%</span>
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

    .row.disabled .label,
    .row.disabled .value {
        color: var(--text-dim);
    }

    .select:disabled,
    .slider:disabled {
        opacity: 0.4;
        cursor: not-allowed;
    }
</style>
