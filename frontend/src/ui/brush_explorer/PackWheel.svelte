<script lang="ts">
    /**
     * The rolodex: one uniform card per visible group, scroll-synced to the
     * brush list beside it.
     *
     * A native `overflow-y: auto` scrollport, so pen and touch momentum come
     * from the platform rather than a hand-rolled inertia integrator. The pads
     * above and below the stack are what let the first and last cards reach the
     * focus line, and they are asymmetric because the line is not the middle.
     * They also give the wheel a scroll range of its own whenever there are two
     * packs: one card of travel per pack, which is what makes it a minimap you
     * can flick through rather than a second copy of the list's scrolling.
     *
     * Bounded, not circular. A wheel that wrapped could not be honestly synced
     * to a list that has a real top and bottom.
     */
    import PackCard from './PackCard.svelte';
    import type { BrushGroup } from '../brush_library/grouping';
    import {
        FLAT_CURVE,
        wheelPadBottom,
        wheelPadTop,
        type CardCurve,
        type WheelGeometry,
    } from './wheel';

    interface Props {
        groups: BrushGroup[];
        geometry: WheelGeometry;
        /** Index of the group under the list's focus line. */
        focused: number | null;
        /**
         * The rolodex transform for each card, computed by the parent's frame
         * loop from the same wheel position it is moving this scrollport to.
         *
         * Deliberately *not* derived here from this element's own `scrollTop`.
         * A card's transform and the scroll position it describes have to be
         * one frame's worth of the same number: a `scroll` event arrives after
         * the write that caused it, so a wheel that styled itself from its own
         * events painted every card tilted for the position it had just left.
         */
        curves: CardCurve[];
        /** Where this scrollport sits in the explorer, px. A card adds its own
         *  position within the column to it to find where it is in the field. */
        paneTop: number;
        /** Where a card's leading edge sits in the explorer, px. */
        paneLeft: number;
        /** Bound so the parent can drive and read this scrollport directly. */
        el: HTMLElement | undefined;
        /** This pane moved; keep the frame loop awake. */
        onScroll: () => void;
        /** The user put a hand on this pane, so it is the one driving now. */
        onDrive: () => void;
        onPick: (index: number) => void;
    }
    let {
        groups,
        geometry,
        focused,
        curves,
        paneTop,
        paneLeft,
        el = $bindable(),
        onScroll,
        onDrive,
        onPick,
    }: Props = $props();
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
    class="pack-wheel"
    bind:this={el}
    style:--pane-top="{paneTop}px"
    style:--pane-left="{paneLeft}px"
    style:padding-top="{wheelPadTop(geometry)}px"
    style:padding-bottom="{wheelPadBottom(geometry)}px"
    onscroll={onScroll}
    onpointerdown={onDrive}
    onwheel={onDrive}
>
    {#each groups as group, i (group.id)}
        <PackCard
            {group}
            active={i === focused}
            curve={curves[i] ?? FLAT_CURVE}
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
         * (`wheelLead`, `wheelScrollMax`) rather than assumed; the two must
         * agree or the sync silently stops moving. */
    }
    .pack-wheel::-webkit-scrollbar {
        display: none;
    }
</style>
