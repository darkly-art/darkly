<script lang="ts">
    import CanvasView from '../canvas/CanvasView.svelte';
    import { shell } from './shell.svelte';
</script>

<!--
    Renders one `<CanvasView>` per open instance. The keyed `{#each}` means
    Svelte mounts each view exactly once; switching tabs only toggles
    visibility, so each instance's canvas (and thus its WebGPU surface and
    bound `DarklyHandle`) survives across tab switches.
-->
{#each shell.instances as inst (inst.id)}
    <div class="canvas-slot" class:hidden={inst.id !== shell.activeId}>
        <CanvasView instance={inst} />
    </div>
{/each}

<style>
    .canvas-slot {
        flex: 1;
        display: flex;
        min-height: 0;
        min-width: 0;
    }
    .canvas-slot.hidden {
        display: none;
    }
</style>
