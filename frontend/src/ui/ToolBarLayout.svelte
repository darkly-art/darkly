<script lang="ts">
    import type { Snippet } from 'svelte';

    let {
        left,
        center,
        right,
    }: {
        left?: Snippet;
        center?: Snippet;
        right?: Snippet;
    } = $props();
</script>

<div class="layout">
    {#if left}{@render left()}{/if}
    <div class="center">
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
</style>
