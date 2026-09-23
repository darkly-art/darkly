<script lang="ts">
    import CanvasStack from './CanvasStack.svelte';
    import { canvasSlot } from './canvasSlot.svelte';
    import { workspaces } from '../ui/workspace/workspaces.svelte';

    // Mounted exactly once at the app root. Holds the persistent WebGPU canvases
    // and positions itself (position:fixed) over wherever the `Document` panel's
    // placeholder currently sits, so the canvas follows the panel as it's tiled
    // without the canvases ever remounting. The rect is measured and owned by
    // `canvasSlot`, which the tool strip also reads. `null` ⇒ no Document panel
    // mounted (hidden).
    let rect = $derived(canvasSlot.rect);

    // CanvasStack must first mount only once the overlay has a real, visible
    // rect; CanvasView sizes its WebGPU surface from getBoundingClientRect on
    // mount, and a 0×0 init (which happens if it mounts while display:none)
    // leaves the surface and initial view fit broken. This latches true on the
    // first valid rect and never flips back, so the canvases mount exactly once
    // and then persist (hidden via display:none when the Document panel is away,
    // never unmounted).
    let everSized = $state(false);
    $effect(() => {
        if (rect && !everSized) everSized = true;
    });

    // Belt-and-suspenders: any tiling mutation (including a gutter drag that
    // moves the slot's position without resizing it) touches the workspace
    // trees. Deep-read them to subscribe, then re-measure after layout settles.
    // This trigger stays here rather than in `canvasSlot` because it subscribes
    // to `workspaces.workspaces`, which is workspace-global rather than
    // element-local; the module would need an `$effect.root` to hold it, which
    // is not worth the lifecycle care for one rAF. This component is mounted
    // once at the app root, so the trigger is always live.
    $effect(() => {
        void $state.snapshot(workspaces.workspaces);
        requestAnimationFrame(() => canvasSlot.reposition());
    });

    // During a tab drag the overlay must not intercept hit-testing, so a panel
    // can be dropped onto the canvas's edges (the Document panel-body sits
    // directly beneath this overlay).
    let interactive = $derived(!workspaces.dragging);
</script>

<!-- Gated on `everSized` for the FIRST mount only (so the canvas inits at a
     real size); once mounted it stays mounted; unmounting would destroy every
     canvas's WebGPU surface. When no Document panel is showing (rect null) the
     overlay is only hidden via display:none, never removed. -->
{#if everSized}
    <div
        class="canvas-overlay"
        style:display={rect ? 'flex' : 'none'}
        style:left="{rect?.left ?? 0}px"
        style:top="{rect?.top ?? 0}px"
        style:width="{rect?.width ?? 0}px"
        style:height="{rect?.height ?? 0}px"
        style:pointer-events={interactive ? 'auto' : 'none'}
    >
        <CanvasStack />
    </div>
{/if}

<style>
    .canvas-overlay {
        position: fixed;
        z-index: 1;
        display: flex;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
    }
</style>
