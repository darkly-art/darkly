<script lang="ts">
    import { exposedDragSpeed } from '../state/brush_graph.svelte';
    import { magicWandSession } from '../tools/magic_wand.svelte';

    const MIN = 0;
    const MAX = 255;
    const DEFAULT = 15;

    let dragging = $state(false);
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
    class="scrub"
    class:dragging
    title="How similar a pixel's color must be to the clicked pixel to be selected (0 = exact match, 255 = anything)."
    onpointerdown={(e) => {
        e.preventDefault();
        const startX = e.clientX;
        const startVal = magicWandSession.tolerance;
        const speed = exposedDragSpeed(MIN, MAX);
        const el = e.currentTarget as HTMLElement;
        el.setPointerCapture(e.pointerId);
        dragging = true;
        const onMove = (ev: PointerEvent) => {
            const dx = ev.clientX - startX;
            const v = Math.round(Math.min(MAX, Math.max(MIN, startVal + dx * speed)));
            magicWandSession.tolerance = v;
        };
        const onUp = () => {
            dragging = false;
            el.removeEventListener('pointermove', onMove);
            el.removeEventListener('pointerup', onUp);
        };
        el.addEventListener('pointermove', onMove);
        el.addEventListener('pointerup', onUp);
    }}
    ondblclick={() => { magicWandSession.tolerance = DEFAULT; }}
>
    <i class="fa-solid fa-sliders scrub-icon"></i>
    <div class="scrub-text">
        <span class="scrub-label">Tolerance</span>
        <span class="scrub-value">{magicWandSession.tolerance}</span>
    </div>
</div>

<style>
    .scrub {
        flex-shrink: 0;
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 4px 10px;
        border-radius: 6px;
        cursor: col-resize;
        background: var(--bg-hover);
        transition: background 0.1s;
    }

    .scrub:hover {
        background: var(--bg-active);
    }

    .scrub.dragging {
        background: var(--accent);
    }

    :global(.scrub.dragging .scrub-icon),
    :global(.scrub.dragging .scrub-label),
    :global(.scrub.dragging .scrub-value) {
        color: #ffffff;
    }

    :global(.scrub-icon) {
        font-size: 14px;
        color: var(--text-muted);
    }

    .scrub-text {
        display: flex;
        flex-direction: column;
    }

    .scrub-label {
        font-size: 9px;
        color: var(--text-muted);
        text-transform: uppercase;
        letter-spacing: 0.5px;
        line-height: 1;
    }

    .scrub-value {
        font-size: 12px;
        color: var(--text);
        font-variant-numeric: tabular-nums;
        line-height: 1.3;
    }
</style>
