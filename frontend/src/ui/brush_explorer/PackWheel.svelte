<script lang="ts">
    /**
     * The rolodex: one uniform card per visible group, scroll-synced to the
     * brush list beside it.
     *
     * A native `overflow-y: auto` scrollport, so pen and touch momentum come
     * from the platform rather than a hand-rolled inertia integrator. Half a
     * viewport of pad above and below the stack keeps the focused card in the
     * middle of the column and gives the wheel a scroll range of its own
     * whenever there are two packs — one card of wheel travel per pack, which
     * is what makes it a minimap you can flick through rather than a second
     * copy of the list's own scrolling.
     *
     * Bounded, not circular. A wheel that wrapped could not be honestly synced
     * to a list that has a real top and bottom.
     */
    import PackCard from './PackCard.svelte';
    import type { BrushGroup } from '../brush_library/grouping';
    import { cardCurve, wheelPad, type WheelGeometry } from './wheel';

    interface Props {
        groups: BrushGroup[];
        geometry: WheelGeometry;
        /** Index of the group under the list's focus line. */
        focused: number | null;
        /** Bound so the parent can drive and read this scrollport directly. */
        el: HTMLElement | undefined;
        onScroll: () => void;
        onPointerDown: () => void;
        onPick: (index: number) => void;
    }
    let { groups, geometry, focused, el = $bindable(), onScroll, onPointerDown, onPick }: Props = $props();

    /** The wheel's own scroll offset, mirrored only to drive `cardCurve`.
     *  Read from the DOM on each scroll rather than written back to it, so this
     *  is not a third participant in the sync. */
    let offset = $state(0);
    function handleScroll() {
        offset = el?.scrollTop ?? 0;
        onScroll();
    }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
    class="pack-wheel"
    bind:this={el}
    style:padding-block="{wheelPad(geometry)}px"
    onscroll={handleScroll}
    onpointerdown={onPointerDown}
>
    {#each groups as group, i (group.id)}
        <PackCard
            {group}
            active={i === focused}
            curve={cardCurve(i, offset, geometry)}
            onSelect={() => onPick(i)}
        />
    {/each}
</div>

<style>
    .pack-wheel {
        display: flex;
        flex-direction: column;
        gap: var(--card-gap, 8px);
        height: 100%;
        min-height: 0;
        overflow-y: auto;
        /* Pen and touch pan this vertically; without it an ancestor's
         * `touch-action: none` (the canvas gesture guard) leaves it scrollable
         * only by wheel. */
        touch-action: pan-y;
        /* An overscroll fling stops here rather than chaining out to the
         * modal or the page behind it. */
        overscroll-behavior: contain;
        padding-inline: 10px;
        scrollbar-width: none;
        /* `measure()` reads each card's `offsetTop` to find the leading pad,
         * and `offsetTop` is relative to the nearest positioned ancestor. */
        position: relative;
        /* The block padding is set inline from `wheelPad`, and every card
         * position the mapping computes is measured back off the DOM
         * (`wheelLead`, `wheelScrollMax`) rather than assumed — the two must
         * agree or the sync silently stops moving. */
    }
    .pack-wheel::-webkit-scrollbar {
        display: none;
    }
</style>
