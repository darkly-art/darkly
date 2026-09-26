<script lang="ts">
    /**
     * The tool options row itself, not a layout helper.
     *
     * It leads with the global foreground/background swatches and then lays
     * the active tool's own controls out in a wrapping flow, with an optional
     * right-aligned group. Every tool options component renders it, and one
     * that hand-rolls its own markup instead silently loses the color chrome.
     *
     * The swatches are a member of this row rather than chrome beside the bar,
     * which is what makes them tile with the tool's controls when the window
     * narrows: a `flex: none` sibling of a wrapping container cannot wrap with
     * that container's children, only next to them.
     */
    import type { Snippet } from 'svelte';
    import FgBgSwatches from './color/FgBgSwatches.svelte';

    let {
        center,
        right,
    }: {
        center?: Snippet;
        right?: Snippet;
    } = $props();
</script>

<div class="layout">
    <div class="center">
        <div class="color-zone">
            <FgBgSwatches mode="popup" />
        </div>
        {#if center}{@render center()}{/if}
    </div>
    {#if right}{@render right()}{/if}
</div>

<style>
    .layout {
        flex: 1;
        display: flex;
        align-items: center;
        gap: 4px;
        min-width: 0;
    }

    /* When the bar is too narrow to hold every control on one line, the
     * controls wrap onto additional lines (growing the bar's height)
     * rather than falling back to a horizontal scrollbar. `min-width: 0`
     * lets this flex child shrink below its content width so the wrap
     * point tracks the available space. */
    .center {
        flex: 1;
        min-width: 0;
        display: flex;
        align-items: center;
        justify-content: flex-start;
        flex-wrap: wrap;
        gap: 4px;
    }

    /* A little more room than the row's own gap, and no rule: a divider that
       tiles with the controls hangs off the end of whichever line it lands on. */
    .color-zone {
        display: flex;
        align-items: center;
        flex: none;
        margin-right: 4px;
    }
</style>
