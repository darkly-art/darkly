<script lang="ts">
    import { app } from '../../state/app.svelte';

    interface VeilParam {
        kind: 'float' | 'int' | 'bool';
        name: string;
        min?: number;
        max?: number;
        default: number | boolean;
        value?: number | boolean;
    }

    let { veil, onupdate }: {
        veil: { type: string; visible: boolean; index: number; params: VeilParam[] };
        onupdate: () => void;
    } = $props();

    let expanded = $state(false);
    let dropPos = $state<'none' | 'above' | 'below'>('none');
    let draggable = $state(true);

    function toggleExpand() {
        if (veil.params.length > 0) expanded = !expanded;
    }

    function toggleVisibility(e: MouseEvent) {
        e.stopPropagation();
        if (app.handle) {
            app.handle.set_veil_visible(veil.index, !veil.visible);
            onupdate();
        }
    }

    function remove(e: MouseEvent) {
        e.stopPropagation();
        if (app.handle) {
            app.handle.remove_veil(veil.index);
            onupdate();
        }
    }

    function onParamChange() {
        if (!app.handle) return;
        const params: Record<string, number | boolean> = {};
        for (const p of veil.params) {
            params[p.name] = p.value ?? p.default;
        }
        app.handle.update_veil(veil.index, params);
        onupdate();
    }

    function onSliderInput(param: VeilParam, e: Event) {
        const target = e.target as HTMLInputElement;
        param.value = param.kind === 'int'
            ? parseInt(target.value, 10)
            : parseFloat(target.value);
        onParamChange();
    }

    function onBoolChange(param: VeilParam, e: Event) {
        param.value = (e.target as HTMLInputElement).checked;
        onParamChange();
    }

    function onDragStart(e: DragEvent) {
        e.dataTransfer?.setData('application/x-veil', String(veil.index));
        if (e.dataTransfer) {
            e.dataTransfer.effectAllowed = 'move';
        }
    }

    function onDragOver(e: DragEvent) {
        if (!e.dataTransfer?.types.includes('application/x-veil')) return;
        e.preventDefault();
        e.stopPropagation();
        e.dataTransfer.dropEffect = 'move';

        const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
        const ratio = (e.clientY - rect.top) / rect.height;
        dropPos = ratio < 0.5 ? 'above' : 'below';
    }

    function onDragLeave(e: DragEvent) {
        const related = e.relatedTarget as Node | null;
        if (!related || !(e.currentTarget as HTMLElement).contains(related)) {
            dropPos = 'none';
        }
    }

    function onDrop(e: DragEvent) {
        e.preventDefault();
        e.stopPropagation();
        dropPos = 'none';
        const draggedIdx = e.dataTransfer?.getData('application/x-veil');
        if (draggedIdx == null || !app.handle) return;
        const from = Number(draggedIdx);
        if (from === veil.index) return;

        const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
        const ratio = (e.clientY - rect.top) / rect.height;
        // UI list is reversed from internal order (top-of-list = highest index),
        // so "above" in the UI = after in internal order, matching layer convention.
        let to = ratio < 0.5 ? veil.index + 1 : veil.index;
        // Adjust for removal shifting indices
        if (from < to) to--;

        app.handle.move_veil(from, to);
        onupdate();
    }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
    class="veil-item"
    class:drop-above={dropPos === 'above'}
    class:drop-below={dropPos === 'below'}
    draggable={draggable ? 'true' : 'false'}
    ondragstart={onDragStart}
    ondragover={onDragOver}
    ondragleave={onDragLeave}
    ondrop={onDrop}
    ondragend={() => { dropPos = 'none'; }}
>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div class="veil-header" onclick={toggleExpand}>
        <button
            class="vis-btn"
            class:hidden={!veil.visible}
            onclick={toggleVisibility}
            title="Toggle visibility"
        >
            <i class={veil.visible ? 'fa-solid fa-eye' : 'fa-solid fa-eye-slash'}></i>
        </button>

        <span class="veil-name">{veil.type}</span>

        {#if veil.params.length > 0}
            <span class="expand-indicator"><i class={expanded ? 'fa-solid fa-chevron-up' : 'fa-solid fa-chevron-down'}></i></span>
        {/if}

        <button
            class="remove-btn"
            onclick={remove}
            title="Remove veil"
        >
            <i class="fa-solid fa-trash"></i>
        </button>
    </div>

    {#if expanded && veil.params.length > 0}
        <div class="veil-params">
            {#each veil.params as param}
                <label class="param-row">
                    <span class="param-label">{param.name}</span>
                    {#if param.kind === 'float' || param.kind === 'int'}
                        <input
                            type="range"
                            class="param-slider"
                            min={param.min}
                            max={param.max}
                            step={param.kind === 'int' ? 1 : ((param.max! - param.min!) / 100)}
                            value={param.value ?? param.default}
                            oninput={(e) => onSliderInput(param, e)}
                            onclick={(e) => e.stopPropagation()}
                            onpointerdown={() => { draggable = false; }}
                            onpointerup={() => { draggable = true; }}
                        />
                        <span class="param-value">
                            {param.kind === 'int' ? (param.value ?? param.default) : ((param.value ?? param.default) as number).toFixed(1)}
                        </span>
                    {:else if param.kind === 'bool'}
                        <input
                            type="checkbox"
                            class="param-checkbox"
                            checked={(param.value ?? param.default) as boolean}
                            onchange={(e) => onBoolChange(param, e)}
                            onclick={(e) => e.stopPropagation()}
                        />
                    {/if}
                </label>
            {/each}
        </div>
    {/if}
</div>

<style>
    .veil-item {
        position: relative;
    }

    .veil-item.drop-above::before {
        content: '';
        position: absolute;
        top: -1px;
        left: 8px;
        right: 4px;
        height: 2px;
        background: var(--accent);
        pointer-events: none;
    }

    .veil-item.drop-below::after {
        content: '';
        position: absolute;
        bottom: -1px;
        left: 8px;
        right: 4px;
        height: 2px;
        background: var(--accent);
        pointer-events: none;
    }

    .veil-header {
        display: flex;
        align-items: center;
        gap: 4px;
        padding: 6px 12px;
        min-height: 24px;
        cursor: pointer;
        transition: background 0.1s;
    }

    .veil-header:hover {
        background: var(--bg-hover);
    }

    .vis-btn {
        background: none;
        border: none;
        color: var(--text-muted);
        cursor: pointer;
        padding: 0;
        font-size: 12px;
        width: 18px;
        text-align: center;
        transition: color 0.1s;
    }
    .vis-btn:hover { color: var(--text); }
    .vis-btn.hidden { color: var(--text-dim); }

    .veil-name {
        flex: 1;
        font-size: 12px;
        color: var(--text);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .expand-indicator {
        font-size: 10px;
        color: var(--text-muted);
        margin-right: 2px;
    }

    .remove-btn {
        background: none;
        border: none;
        color: var(--text-dim);
        cursor: pointer;
        padding: 0;
        font-size: 14px;
        width: 18px;
        text-align: center;
        line-height: 1;
        transition: color 0.1s;
    }
    .remove-btn:hover { color: var(--danger); }

    .veil-params {
        padding: 4px 8px 8px 28px;
        display: flex;
        flex-direction: column;
        gap: 4px;
    }

    .param-row {
        display: flex;
        align-items: center;
        gap: 6px;
    }

    .param-label {
        font-size: 11px;
        color: var(--text-muted);
        min-width: 40px;
    }

    .param-slider {
        flex: 1;
        height: 4px;
    }

    .param-value {
        font-size: 10px;
        color: var(--text-muted);
        min-width: 28px;
        text-align: right;
        font-variant-numeric: tabular-nums;
    }

    .param-checkbox {
        accent-color: var(--accent);
    }
</style>
