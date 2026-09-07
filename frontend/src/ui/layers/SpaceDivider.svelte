<script lang="ts">
    import { layerDropTarget } from './dropTarget.svelte';
    import Icon from '../../icons/Icon.svelte';

    /** The divider row. An ordinary tree node: dragging it issues the same
     *  `moveLayers` call as any row (an illegal drop surfaces the engine's
     *  refusal as a toast), and its two halves are drop targets for the two
     *  spaces it separates — above it is viewport-only, below it is canvas.
     *  `select: false` keeps a grab from touching the layer selection. */
    let {
        divider,
        empty = false,
        onupdate,
    }: { divider: { id: number }; empty?: boolean; onupdate?: () => void } = $props();
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
    class="divider"
    class:empty
    title="Effects above this line change how the canvas looks on screen. They are not part of the image — exports, Flatten and Merge ignore them."
    draggable="true"
    use:layerDropTarget={{
        rowId: divider.id,
        draggable: true,
        select: false,
        onupdate: () => onupdate?.(),
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
