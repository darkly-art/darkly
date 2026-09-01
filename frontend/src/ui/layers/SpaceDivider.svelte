<script lang="ts">
    import { app } from '../../state/app.svelte';
    import { pointerDrag } from '../workspace/pointerDrag';

    /** Pixel height of one layer row, for translating a drag into a row count. */
    const ROW_HEIGHT = 34;

    let { onupdate }: { onupdate?: () => void } = $props();

    /** Live count during a drag; `null` when resting on the engine's value. */
    let dragCount = $state<number | null>(null);
    let dragStart = 0;

    let count = $derived(dragCount ?? app.screenSpaceCount);

    /** How far down the divider can go: it stops at the first row that cannot
     *  be rendered after the view transform. The engine clamps authoritatively
     *  when the drag lands; this is what keeps the handle from visibly
     *  overshooting under the cursor. */
    let maxCount = $derived.by(() => {
        let n = 0;
        for (const row of app.layerTree) {
            if (!row.screenSpaceEligible) break;
            n++;
        }
        return n;
    });

    function onMove(_dx: number, dy: number) {
        const rows = Math.round((dy + dragStart) / ROW_HEIGHT);
        dragCount = Math.max(0, Math.min(maxCount, rows));
    }

    function onEnd(aborted: boolean) {
        const landed = dragCount;
        dragCount = null;
        if (aborted || landed === null || landed === app.screenSpaceCount) return;
        app.setScreenSpaceBoundary(landed);
        onupdate?.();
    }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
    class="divider"
    class:empty={count === 0}
    style:top="{count * ROW_HEIGHT}px"
    title="Anything above this line is applied to your view of the canvas. It is not part of the image: exports, Flatten and Merge ignore it."
    use:pointerDrag={{
        onStart: () => {
            dragStart = app.screenSpaceCount * ROW_HEIGHT;
            dragCount = app.screenSpaceCount;
        },
        onMove,
        onEnd,
    }}
>
    <span class="rule"></span>
    <span class="label">
        Viewport only — not exported
        {#if count === 0}<span class="hint">Drag down to make an effect viewport-only</span>{/if}
    </span>
    <span class="rule"></span>
</div>

<style>
    .divider {
        position: absolute;
        left: 0;
        right: 0;
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 0 8px;
        height: 12px;
        margin-top: -6px;
        cursor: ns-resize;
        user-select: none;
        z-index: 2;
    }

    .rule {
        flex: 1;
        height: 1px;
        background: var(--border-color, #444);
    }

    .label {
        font-size: 9px;
        letter-spacing: 0.04em;
        text-transform: uppercase;
        color: var(--text-dim, #888);
        white-space: nowrap;
    }

    /* At rest on an empty run the line is decoration, not a boundary with
       anything above it, so it recedes until hovered. */
    .divider.empty .label,
    .divider.empty .rule {
        opacity: 0.45;
    }

    .divider:hover .label,
    .divider:hover .rule {
        opacity: 1;
    }

    .hint {
        display: none;
        margin-left: 6px;
        text-transform: none;
        letter-spacing: 0;
    }

    .divider.empty:hover .hint {
        display: inline;
    }
</style>
