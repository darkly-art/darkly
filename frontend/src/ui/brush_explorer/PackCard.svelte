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
    import { CARD_PERSPECTIVE, type CardCurve } from './wheel';

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
    class="pack-card pack-lit pack-rim"
    aria-current={active}
    onclick={onSelect}
    use:packPalette={group.palette}
    style:--card-pane-y="{curve.paneY}px"
    style:transform="perspective({CARD_PERSPECTIVE}px) rotateX({curve.rotateX}deg) scale({curve.scale})"
    style:opacity={curve.opacity}
    title={group.pack?.description || group.label}
>
    <span class="face">
        <Icon name={group.icon} class="card-icon" />
        <span class="label">{group.label}</span>
        <span class="count">{group.brushes.length}</span>
    </span>
</button>

<style>
    /* The left end of the pack: `surface` filling it, and the vivid pair spent
     * on the light that catches its edge and on the name written across it. */
    .pack-card {
        display: flex;
        align-items: center;
        width: 100%;
        padding: 10px 12px;
        font-family: inherit;
        font-size: 12px;
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
    /* Where this card samples the field.
     *
     * The field belongs to the explorer, not to this card: sized to the
     * explorer and offset by where the card currently is inside it — the
     * wheel's own offset plus the card's position in the column — so it
     * resolves to the same place on screen as the ribbon and the section
     * painting the identical declaration. Scrolling slides the cards under a
     * light that stays put, and a pack's three columns are one continuous
     * surface rather than three that match.
     *
     * `--card-pane-y` is `CardCurve.paneY`, published every frame by the same
     * loop that sets the rolodex transform, so the light and the tilt are
     * always one frame's worth of the same number.
     *
     * A card cannot hold the field still the way a section does
     * (`background-attachment: fixed`, `BrushExplorer.svelte`). A transformed
     * element is the containing block for its own fixed background, and the
     * rolodex curve above transforms every card — so `fixed` here re-anchors
     * the image to the card, which is precisely the "paint travels with the
     * card" that the offset exists to avoid. Worse, the image is sized to the
     * explorer and offset by the explorer's viewport origin, so it lands
     * entirely outside the card's box and the card paints as bare ink on
     * nothing.
     *
     * The offset is exact whenever the *list* is the driver, because the wheel
     * is then the pane the frame loop writes, and a written position and the
     * values derived from it land in one style commit. It is a frame stale only
     * while the wheel itself is under the hand.
     *
     * No border on the trailing edge, so no rim there: that edge is where the
     * projection leaves, and it is interior to the pack. */
    .pack-card::before {
        --pack-field-offset:
            calc(-1 * var(--pane-left, 0px))
            calc(-1 * (var(--pane-top, 0px) + var(--card-pane-y, 0px)));
        background-position: var(--pack-field-offset);
        border-right-width: 0;
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

    /* The card's name, written in the vivid pair rather than in an ink: chroma
     * where the icon is and refraction by the time the count is reached, so the
     * text crosses the same two colours in the same direction the beam does.
     *
     * This is the pack's own colour on the pack's own surface with nothing
     * neutral in between, which a saturated pair can carry and a tinted
     * near-white cannot — the vivid pair reads against a light theme and a dark
     * one alike, so the card needs no colour picked for one of them.
     *
     * A box of its own because `background-clip: text` claims the element's
     * background, and the card's is spoken for twice over — surface beneath and
     * beam above. Here there is nothing else to spend it on. */
    .face {
        display: flex;
        align-items: center;
        gap: 8px;
        width: 100%;
        min-width: 0;
        background-image: linear-gradient(90deg, var(--pack-chroma), var(--pack-refraction));
        -webkit-background-clip: text;
        background-clip: text;
        color: transparent;
    }
    /* The icon is an SVG and takes `currentColor`, which the clip above has
     * emptied for the text's sake. It sits at the gradient's own left end, so
     * naming that colour outright is what the ramp would have given it. */
    .pack-card :global(.card-icon) {
        font-size: 13px;
        flex: none;
        color: var(--pack-chroma);
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
    /* The same gradient, held back — a count is a footnote on the label, and any
     * separate grey would fight whatever colour the pack brought.
     *
     * Held back by weight rather than by opacity: the colour here is painted by
     * `.face`'s background through these glyphs, so fading *this* box fades a
     * layer the colour does not live on. Size and weight are the count's own. */
    .count {
        flex: none;
        font-size: 10px;
        font-weight: 300;
        font-variant-numeric: tabular-nums;
    }
</style>
