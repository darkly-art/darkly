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

    /* Center is the only scrollable region. `min-width: 0` is required
     * for a flex child to be allowed to shrink below its content size —
     * without it the parent grows and the bar overflows its column
     * instead of letting this region scroll. */
    .center {
        flex: 1;
        min-width: 0;
        display: flex;
        align-items: center;
        gap: 4px;
        overflow-x: auto;
        overflow-y: hidden;
        scrollbar-width: thin;
    }
</style>
