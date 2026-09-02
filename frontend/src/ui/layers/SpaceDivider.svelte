<script lang="ts">
    import { app } from '../../state/app.svelte';
    import { pointerDrag } from '../workspace/pointerDrag';
    import { gapAt, maxEligible } from './spaceDivider';
    import Icon from '../../icons/Icon.svelte';

    let { onupdate }: { onupdate?: () => void } = $props();

    let el = $state<HTMLElement | null>(null);
    /** Live count during a drag; `null` when resting on the engine's value. */
    let dragCount = $state<number | null>(null);

    let count = $derived(dragCount ?? app.screenSpaceCount);

    let maxCount = $derived(maxEligible(app.layerTree));

    /** Which gap the pointer is nearest, measured against the rows that are
     *  actually on screen. Rows are siblings in the list; this one is too, so
     *  it skips itself. */
    function countAt(clientY: number): number {
        const list = el?.parentElement;
        if (!list) return count;
        const rows = [...list.children]
            .filter(child => child !== el)
            .map(child => {
                const rect = child.getBoundingClientRect();
                return { top: rect.top, height: rect.height };
            });
        return gapAt(clientY, rows, maxCount);
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
    bind:this={el}
    class="divider"
    class:empty={count === 0}
    title="Effects above this line change how the canvas looks on screen. They are not part of the image — exports, Flatten and Merge ignore them."
    use:pointerDrag={{
        onStart: (e) => { dragCount = countAt(e.clientY); },
        onMove: (_dx, _dy, e) => { dragCount = countAt(e.clientY); },
        onEnd,
    }}
>
    <span class="rule"></span>
    <span class="label"><Icon name="fa6-solid:display" /> viewport</span>
    <span class="rule"></span>
</div>

<style>
    .divider {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 0 8px;
        height: 14px;
        flex: 0 0 auto;
        cursor: ns-resize;
        user-select: none;
    }

    .rule {
        flex: 1;
        height: 1px;
        background: var(--border-color, #444);
    }

    .label {
        display: inline-flex;
        align-items: center;
        gap: 4px;
        font-size: 9px;
        letter-spacing: 0.06em;
        color: var(--text-dim, #888);
        white-space: nowrap;
    }

    /* With nothing above it the line is an affordance, not a boundary, so it
       recedes until pointed at. */
    .divider.empty {
        opacity: 0.4;
    }

    .divider:hover {
        opacity: 1;
    }
</style>
