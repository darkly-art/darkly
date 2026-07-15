<script lang="ts">
    import ParamRow from './ParamRow.svelte';
    import {
        listItemSchema,
        newListEntry,
        cloneParamValue,
        type FilterParam,
        type FilterParamValue,
        type ListValue,
    } from './filterParams';

    // Generic editor for a `list` param — a dynamic list of homogeneous entries,
    // each a bordered group of `ParamRow`s bound into its `{ name: value }`
    // record. Add appends `newListEntry()`; per-entry Remove drops it. "Add" is
    // disabled at the schema's `max` cap (no silently-dropped entries, no
    // effect-specific constant here). Every mutation fires the standard
    // `oninput`/`onchange` contract, so the destructive preview, filter-layer,
    // and veil surfaces need no changes.
    type Props = {
        param: FilterParam;
        oninput?: () => void;
        onchange?: () => void;
    };
    let { param, oninput, onchange }: Props = $props();

    const schema = $derived(listItemSchema(param));
    const maxLen = $derived(param.max ?? Infinity);
    const entries = $derived((param.value ?? param.default ?? []) as ListValue);

    // Ensure `param.value` is an editable array we own (deep-cloned from the
    // shared schema default on first edit), then return it for mutation.
    function owned(): ListValue {
        if (!Array.isArray(param.value)) {
            param.value = cloneParamValue((param.default ?? []) as ListValue);
        }
        return param.value as ListValue;
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
        onchange?.();
    }

    function removeEntry(i: number) {
        const arr = owned();
        arr.splice(i, 1);
        param.value = [...arr];
        onchange?.();
    }
</script>

<div class="list-editor">
    <div class="list-head">
        <span class="label">{param.name}</span>
        <span class="count">{entries.length}{maxLen !== Infinity ? ` / ${maxLen}` : ''}</span>
    </div>

    {#each entries as entry, i (i)}
        <div class="entry">
            <div class="entry-head">
                <span class="entry-title">#{i + 1}</span>
                <button class="remove" title="Remove entry" onclick={() => removeEntry(i)}>
                    ✕
                </button>
            </div>
            {#each schema as def (def.name)}
                {@const field = { ...def, value: entry[def.name] ?? def.default }}
                <ParamRow
                    param={field}
                    oninput={() => writeField(i, def.name, field.value as FilterParamValue, false)}
                    onchange={() => writeField(i, def.name, field.value as FilterParamValue, true)}
                />
            {/each}
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
    }
    .entry-head {
        display: flex;
        align-items: center;
        justify-content: space-between;
    }
    .entry-title {
        font-size: 11px;
        color: var(--text-dim);
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
