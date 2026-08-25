<script lang="ts">
    /**
     * A grid of every icon in the offline bundle, one of which is selected.
     *
     * `BUNDLED_ICON_NAMES` is what `scripts/gen-icon-bundle.mjs` scraped out of
     * this repository's sources, so everything offered here renders by
     * construction — there is nothing to validate and no engine call to make.
     * Adding a choice is one string literal somewhere plus `npm run gen:icons`.
     */
    import Icon from '../icons/Icon.svelte';
    import { BUNDLED_ICON_NAMES } from '../icons/bundle.generated';

    interface Props {
        /** The chosen icon name. `''` when `allowNone` and nothing is picked. */
        value: string;
        /** Offer a "None" cell. Off by default: most things that carry an icon
         *  require one. */
        allowNone?: boolean;
    }
    let { value = $bindable(), allowNone = false }: Props = $props();
</script>

<div class="icon-picker">
    {#if allowNone}
        <button
            type="button"
            class="icon-cell none"
            class:selected={!value}
            onclick={() => (value = '')}
            title="No icon"
        >None</button>
    {/if}
    {#each BUNDLED_ICON_NAMES as name (name)}
        <button
            type="button"
            class="icon-cell"
            class:selected={value === name}
            onclick={() => (value = name)}
            title={name}
        >
            <Icon {name} />
        </button>
    {/each}
</div>

<style>
    .icon-picker {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(34px, 1fr));
        gap: 4px;
        max-height: 180px;
        overflow-y: auto;
        padding: 6px;
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        /* Let a pen scroll the grid: an ancestor's `touch-action: none`
         * (the canvas gesture guard) otherwise leaves it unscrollable with
         * anything but a wheel. */
        touch-action: pan-y;
    }
    .icon-cell {
        display: flex;
        align-items: center;
        justify-content: center;
        height: 34px;
        font-size: 15px;
        color: var(--text);
        background: transparent;
        border: 1px solid transparent;
        border-radius: 4px;
        cursor: pointer;
        font-family: inherit;
    }
    .icon-cell:hover {
        background: var(--bg-hover);
    }
    .icon-cell.selected {
        border-color: var(--accent);
        color: var(--accent);
    }
    .icon-cell.none {
        font-size: 10px;
        color: var(--text-muted);
    }
</style>
