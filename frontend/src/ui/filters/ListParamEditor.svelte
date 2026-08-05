<script lang="ts">
    import ParamRow from './ParamRow.svelte';
    import Icon from '../../icons/Icon.svelte';
    import { rgb01ToHex } from '../../lib/color';
    import {
        listItemSchema,
        newListEntry,
        cloneParamValue,
        channelLabel,
        type ParamInfo,
        type FilterParamValue,
        type ListValue,
        type ColorValue,
    } from './filterParams';

    // Generic editor for a `list` param — a dynamic list of homogeneous entries,
    // each a bordered group of `ParamRow`s bound into its `{ name: value }`
    // record. Add appends `newListEntry()`; per-entry Remove drops it. "Add" is
    // disabled at the schema's `max` cap (no silently-dropped entries, no
    // effect-specific constant here). Every mutation fires the standard
    // `oninput`/`onchange` contract, so the destructive preview, filter-layer,
    // and veil surfaces need no changes.
    //
    // Each entry is independently collapsible (collapsed by default) and can be
    // drag-reordered by its header. Collapse is ephemeral UI state kept in a
    // parallel array that moves/splices in lockstep with the entries.
    type Props = {
        param: ParamInfo;
        oninput?: () => void;
        onchange?: () => void;
    };
    let { param, oninput, onchange }: Props = $props();

    const schema = $derived(listItemSchema(param));
    const maxLen = $derived(param.max ?? Infinity);
    const entries = $derived((param.value ?? param.default ?? []) as ListValue);

    // Parallel to `entries`; a missing/false slot means collapsed (the default).
    let expanded = $state<boolean[]>([]);

    // The first `color`-kind field in the schema, if any — its per-entry value
    // becomes the header swatch (falls back to the entry index when absent).
    const colorField = $derived(schema.find((d) => d.kind === 'color')?.name);

    function entryHex(entry: Record<string, FilterParamValue>): string | null {
        if (!colorField) return null;
        const c = (entry[colorField] ?? schema.find((d) => d.name === colorField)?.default) as
            | ColorValue
            | undefined;
        return Array.isArray(c) ? rgb01ToHex(c) : null;
    }

    let dragIndex = $state<number | null>(null);
    let dropIndex = $state<number | null>(null);
    let dropPos = $state<'above' | 'below'>('above');

    // Ensure `param.value` is an editable array we own (deep-cloned from the
    // shared schema default on first edit), then return it for mutation.
    function owned(): ListValue {
        if (!Array.isArray(param.value)) {
            param.value = cloneParamValue((param.default ?? []) as ListValue);
        }
        return param.value as ListValue;
    }

    function toggle(i: number) {
        const next = [...expanded];
        next[i] = !(next[i] ?? false);
        expanded = next;
    }

    function writeField(i: number, name: string, value: FilterParamValue, commit: boolean) {
        const arr = owned();
        arr[i] = { ...arr[i], [name]: value };
        param.value = arr;
        if (commit) onchange?.();
        else oninput?.();
    }

    function addEntry() {
        const arr = owned();
        if (arr.length >= maxLen) return;
        param.value = [...arr, newListEntry(schema)];
        expanded = [...expanded.slice(0, arr.length), true];
        onchange?.();
    }

    function removeEntry(i: number) {
        const arr = owned();
        arr.splice(i, 1);
        param.value = [...arr];
        const e = [...expanded];
        e.splice(i, 1);
        expanded = e;
        onchange?.();
    }

    // Move the entry (and its collapse state) from `from` to `to`.
    function move(from: number, to: number) {
        if (from === to) return;
        const arr = owned();
        const [item] = arr.splice(from, 1);
        arr.splice(to, 0, item);
        param.value = [...arr];
        const e = [...expanded];
        const [ev] = e.splice(from, 1);
        e.splice(to, 0, ev ?? false);
        expanded = e;
        onchange?.();
    }

    function onDragOver(i: number, e: DragEvent) {
        if (dragIndex === null) return;
        e.preventDefault();
        const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
        dropPos = (e.clientY - rect.top) / rect.height < 0.5 ? 'above' : 'below';
        dropIndex = i;
    }

    function onDrop(i: number) {
        if (dragIndex === null) return;
        let to = dropPos === 'above' ? i : i + 1;
        if (dragIndex < to) to -= 1;
        move(dragIndex, to);
        dragIndex = null;
        dropIndex = null;
    }

    function endDrag() {
        dragIndex = null;
        dropIndex = null;
    }
</script>

<div class="list-editor">
    <div class="list-head">
        <span class="label" title={param.description ?? undefined}
            >{param.label ?? channelLabel(param.name)}</span
        >
        <span class="count">{entries.length}{maxLen !== Infinity ? ` / ${maxLen}` : ''}</span>
    </div>

    {#each entries as entry, i (i)}
        {@const isOpen = expanded[i] ?? false}
        {@const hex = entryHex(entry)}
        <div
            class="entry"
            class:drop-above={dropIndex === i && dropPos === 'above'}
            class:drop-below={dropIndex === i && dropPos === 'below'}
            class:dragging={dragIndex === i}
            ondragover={(e) => onDragOver(i, e)}
            ondrop={() => onDrop(i)}
            role="listitem"
        >
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
                class="entry-head"
                draggable="true"
                ondragstart={() => (dragIndex = i)}
                ondragend={endDrag}
                onclick={() => toggle(i)}
                title={isOpen ? 'Collapse' : 'Expand'}
            >
                <span class="grip" title="Drag to reorder">
                    <Icon name="fa6-solid:grip-vertical" />
                </span>
                <button
                    class="chevron"
                    onclick={(e) => { e.stopPropagation(); toggle(i); }}
                    title={isOpen ? 'Collapse' : 'Expand'}
                >
                    <Icon name={isOpen ? 'fa6-solid:chevron-down' : 'fa6-solid:chevron-right'} />
                </button>
                {#if hex}
                    <span class="swatch" style="background: {hex}"></span>
                    <span class="entry-title"></span>
                {:else}
                    <span class="entry-title">#{i + 1}</span>
                {/if}
                <button
                    class="remove"
                    title="Remove entry"
                    onclick={(e) => { e.stopPropagation(); removeEntry(i); }}
                >
                    ✕
                </button>
            </div>
            {#if isOpen}
                {#each schema as def (def.name)}
                    {@const field = { ...def, value: entry[def.name] ?? def.default }}
                    <ParamRow
                        param={field}
                        oninput={() => writeField(i, def.name, field.value as FilterParamValue, false)}
                        onchange={() => writeField(i, def.name, field.value as FilterParamValue, true)}
                    />
                {/each}
            {/if}
        </div>
    {/each}

    <button class="add" disabled={entries.length >= maxLen} onclick={addEntry}>
        + Add
    </button>
</div>

<style>
    .list-editor {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }
    .list-head {
        display: flex;
        align-items: center;
        justify-content: space-between;
    }
    .label {
        font-size: 11px;
        color: var(--text-muted);
        text-transform: capitalize;
    }
    .count {
        font-size: 11px;
        color: var(--text-dim);
        font-variant-numeric: tabular-nums;
    }
    .entry {
        display: flex;
        flex-direction: column;
        gap: 6px;
        padding: 8px;
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-sm);
        position: relative;
    }
    .entry.dragging {
        opacity: 0.5;
    }
    /* Drop indicator lines drawn on the hovered entry's edges. */
    .entry.drop-above::before,
    .entry.drop-below::after {
        content: '';
        position: absolute;
        left: 0;
        right: 0;
        height: 2px;
        background: var(--accent);
        pointer-events: none;
    }
    .entry.drop-above::before {
        top: -3px;
    }
    .entry.drop-below::after {
        bottom: -3px;
    }
    .entry-head {
        display: flex;
        align-items: center;
        gap: 4px;
        cursor: pointer;
    }
    .grip {
        display: flex;
        align-items: center;
        color: var(--text-dim);
        cursor: grab;
        font-size: 10px;
    }
    .grip:hover {
        color: var(--text-muted);
    }
    .chevron {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 14px;
        background: none;
        border: none;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 9px;
        padding: 0;
    }
    .chevron:hover {
        color: var(--text);
    }
    .swatch {
        width: 12px;
        height: 12px;
        border-radius: var(--radius-sm);
        border: 1px solid var(--bg-hover);
        flex-shrink: 0;
    }
    .entry-title {
        flex: 1;
        font-size: 11px;
        color: var(--text-dim);
        font-variant-numeric: tabular-nums;
        text-transform: uppercase;
    }
    .remove {
        background: transparent;
        border: none;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 11px;
        padding: 2px 4px;
        border-radius: var(--radius-sm);
    }
    .remove:hover {
        background: var(--bg-hover);
        color: var(--text);
    }
    .add {
        align-self: flex-start;
        padding: 4px 12px;
        background: var(--bg-hover);
        border: 1px solid transparent;
        border-radius: var(--radius-sm);
        color: var(--text-muted);
        cursor: pointer;
        font-size: 11px;
    }
    .add:hover:not(:disabled) {
        border-color: var(--accent);
        color: var(--text);
    }
    .add:disabled {
        opacity: 0.4;
        cursor: not-allowed;
    }
</style>
