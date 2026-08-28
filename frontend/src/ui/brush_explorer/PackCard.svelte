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
    import type { CardCurve } from './wheel';

    interface Props {
        group: BrushGroup;
        /** The group currently under the list's focus line. */
        active: boolean;
        /** Rolodex transform and pane position, from `cardCurve`. */
        curve: CardCurve;
        onSelect: () => void;
    }
    let { group, active, curve, onSelect }: Props = $props();
</script>

<button
    class="pack-card"
    aria-current={active}
    onclick={onSelect}
    use:packPalette={group.palette}
    style:--card-pane-y="{curve.paneY}px"
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
     * the vivid pair spent entirely on the light that plays across it. */
    .pack-card {
        display: flex;
        align-items: center;
        gap: 8px;
        width: 100%;
        padding: 10px 12px;
        font-family: inherit;
        font-size: 12px;
        /* The light belongs to the column, not to the card.
         *
         * The image is sized to the whole scrollport and offset by where this
         * card currently sits in it, so it resolves to the same place on screen
         * whatever the card is doing. Scrolling therefore slides the cards
         * *under* a light that stays put: one brightens as it comes up to the
         * focus line, dims as it leaves. Paint anchored to the card instead
         * looks identical still or flying, which is why nothing before this read
         * as a surface catching light rather than as a coloured rectangle.
         *
         * `--card-pane-y` is `CardCurve.paneY`, published every frame by the
         * same loop that sets the rolodex transform, so the light and the tilt
         * are always one frame's worth of the same number. */
        background-color: var(--pack-surface);
        background-image: radial-gradient(
            120% 42% at 50% 50%,
            color-mix(in srgb, var(--pack-refraction) 42%, transparent) 0%,
            color-mix(in srgb, var(--pack-chroma) 16%, transparent) 45%,
            transparent 78%
        );
        background-repeat: no-repeat;
        background-size: 100% var(--wheel-height, 100%);
        background-position: 0 calc(-1 * var(--card-pane-y, 0px));
        color: var(--pack-ink);
        text-align: left;
        border: none;

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
