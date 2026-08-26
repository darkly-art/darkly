<script lang="ts">
    /**
     * One card on the rolodex. A pack, or a derived group like Recents.
     *
     * Reads `group.pack` and never `group.id`: the id is a list key, and asking
     * "which pack is this" to decide what to render is the consumer-side
     * classification the permission booleans exist to make unnecessary.
     */
    import Icon from '../../icons/Icon.svelte';
    import { packPalette } from '../../lib/packPalette';
    import type { BrushGroup } from '../brush_library/grouping';

    interface Props {
        group: BrushGroup;
        /** The group currently under the list's focus line. */
        active: boolean;
        /** Rolodex transform, from `cardCurve`. */
        curve: { rotateX: number; scale: number; opacity: number };
        onSelect: () => void;
    }
    let { group, active, curve, onSelect }: Props = $props();
</script>

<button
    class="pack-card"
    aria-current={active}
    onclick={onSelect}
    use:packPalette={group.palette}
    style:transform="perspective(420px) rotateX({curve.rotateX}deg) scale({curve.scale})"
    style:opacity={curve.opacity}
    title={group.pack?.description || group.label}
>
    <Icon name={group.icon} class="card-icon" />
    <span class="label">{group.label}</span>
    <span class="count">{group.brushes.length}</span>
</button>

<style>
    /* The left end of the pack: `surface` filling it, `ink` written on it, and
     * the two vivid colours at the top edge only — 2px of chroma with a 1px
     * refraction strand beneath it.
     *
     * The right edge carries no strand: the ribbon leaves from there, so that
     * edge is interior to the pack rather than part of its outline. Same reason
     * the corner there is square. */
    .pack-card {
        display: flex;
        align-items: center;
        gap: 8px;
        width: 100%;
        padding: 10px 12px;
        font-family: inherit;
        font-size: 12px;
        background: var(--pack-surface);
        color: var(--pack-ink);
        text-align: left;
        border: none;
        /* Drawn entirely with inset shadows, never borders or outlines that
         * occupy space. This pane measures its own layout every frame and feeds
         * the result back into the wheel's padding and the scroll mapping, so
         * decoration that changes a height or a width by even 1px can flip the
         * brush grid's column count, which changes the section height, which
         * toggles the list's scrollbar, which changes the width back — an
         * oscillation with no fixed point, and the wheel twitching in sympathy
         * because it is slaved to these numbers. A shadow costs no layout, so it
         * cannot start one.
         *
         * Painted first-on-top: 2px of chroma across the head, then refraction
         * 3px deep showing only as the 1px beneath it. */
        box-shadow:
            inset 0 2px 0 0 var(--pack-chroma),
            inset 0 3px 0 0 var(--pack-refraction);
        /* Square where the projection leaves. A rounded corner there would cut
         * the colour away from the band at exactly the join, which is the one
         * edge that has to be flat for the two to meet. */
        border-radius: var(--radius-md) 0 0 var(--radius-md);
        cursor: pointer;
        /* The curve is applied per card from `cardCurve`, anchored to the
         * trailing edge. Cards recede toward the ends of the column but their
         * right edges stay on one vertical line — which is what the projection
         * leaves from, so it can meet every card at a fixed x instead of
         * chasing an edge that moves with the scale. Anchoring the origin here
         * is why that is true rather than approximately true. */
        transform-origin: right center;
        will-change: transform, opacity;
        transition: filter var(--transition-fast);
    }
    /* A lift rather than a second colour: every surface here is the pack's own,
     * and brightening keeps it that way whatever the pack brought. */
    .pack-card:hover {
        filter: brightness(1.15);
    }
    /* The focused card wears no ring. It is already the only card at full
     * colour and full scale, and the projection leaves from its edge — a
     * border would draw a line exactly where the strands have to run
     * uninterrupted into the list. `aria-current` carries the fact instead. */
    .pack-card :global(.card-icon) {
        font-size: 13px;
        flex: none;
    }
    .label {
        flex: 1;
        min-width: 0;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 0.4px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    /* Same ink, held back — a count is a footnote on the label, and any
     * separate grey would fight whatever colour the pack brought. */
    .count {
        flex: none;
        font-size: 10px;
        opacity: 0.6;
        font-variant-numeric: tabular-nums;
    }
</style>
