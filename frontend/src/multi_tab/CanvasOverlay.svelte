<script lang="ts">
    import CanvasStack from './CanvasStack.svelte';
    import { canvasSlot } from './canvasSlot.svelte';
    import { workspaces } from '../ui/workspace/workspaces.svelte';

    // Mounted exactly once at the app root. Holds the persistent WebGPU canvases
    // and positions itself (position:fixed) over wherever the `Document` panel's
    // placeholder currently sits, so the canvas follows the panel as it's tiled
    // without the canvases ever remounting. `null` rect ⇒ no Document panel
    // mounted (hidden).
    let rect = $state<{ left: number; top: number; width: number; height: number } | null>(null);

    // CanvasStack must first mount only once the overlay has a real, visible
    // rect — CanvasView sizes its WebGPU surface from getBoundingClientRect on
    // mount, and a 0×0 init (which happens if it mounts while display:none)
    // leaves the surface and initial view fit broken. This latches true on the
    // first valid rect and never flips back, so the canvases mount exactly once
    // and then persist (hidden via display:none when the Document panel is away,
    // never unmounted).
    let everSized = $state(false);
    $effect(() => {
        if (rect && !everSized) everSized = true;
    });

    function reposition() {
        const el = canvasSlot.current;
        if (!el) {
            rect = null;
            return;
        }
        const r = el.getBoundingClientRect();
        rect = { left: r.left, top: r.top, width: r.width, height: r.height };
    }

    // Track the current placeholder: reposition on its resize (covers gutter
    // drags, which resize the slot) and on window resize. Re-runs when the
    // placeholder element itself changes — i.e. when the Document panel remounts
    // in a new spot after being moved/tiled.
    $effect(() => {
        const el = canvasSlot.current;
        if (!el) {
            rect = null;
            return;
        }
        reposition();
        const ro = new ResizeObserver(() => reposition());
        ro.observe(el);
        window.addEventListener('resize', reposition);
        return () => {
            ro.disconnect();
            window.removeEventListener('resize', reposition);
        };
    });

    // Belt-and-suspenders: any tiling mutation (including a gutter drag that
    // moves the slot's position without resizing it) touches the workspace
    // trees. Deep-read them to subscribe, then reposition after layout settles.
    $effect(() => {
        void $state.snapshot(workspaces.workspaces);
        requestAnimationFrame(reposition);
    });

    // During a tab drag the overlay must not intercept hit-testing, so a panel
    // can be dropped onto the canvas's edges (the Document panel-body sits
    // directly beneath this overlay).
    let interactive = $derived(!workspaces.dragging);
</script>

<!-- Gated on `everSized` for the FIRST mount only (so the canvas inits at a
     real size); once mounted it stays mounted — unmounting would destroy every
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
