<script lang="ts">
    /**
     * The bands joining each pack's card to the brushes it holds: one per pack
     * currently on screen.
     *
     * **One per pack, not one for the focused pack.** A single band would have
     * to change hands whenever the focus did, and the two cards it would move
     * between are a whole card apart at that moment: every pack boundary would
     * flick the band across that gap. Drawing them all makes the transition
     * something that cannot happen: a pack scrolling out has its band shrink
     * to nothing as its last row leaves, while the next one's grows from
     * nothing, and no band is ever anywhere it was not a frame ago.
     *
     * Each band leaves its card at card height and arrives at its section at
     * section height, so the wheel's compression is something you watch happen
     * rather than something you have to infer.
     *
     * **Clipped divs rather than SVG paths.** A band is the middle of a pack,
     * and the card and section either side of it paint `--pack-beam`, which is
     * a CSS background. An SVG paint server cannot read one, so an SVG band
     * would mean writing the field a second time as `<stop>` elements and
     * keeping the two in step by hand: the duplication that guarantees a
     * visible seam the moment they drift. A div takes the same declaration
     * verbatim: same image, same size, same origin, so the three columns are one
     * surface and the joins need nothing done to them.
     *
     * Purely decorative, hence `aria-hidden` and no pointer events: the same
     * relation is already in the reading order, since the card names the pack
     * and the section follows it.
     */
    import { packPalette } from '../../lib/packPalette';
    import { ribbonCorePath, ribbonPath, ribbonRimPath, type PackBand } from './wheel';

    interface Props {
        bands: PackBand[];
    }
    let { bands }: Props = $props();
</script>

<div class="projection" aria-hidden="true">
    {#each bands as band (band.id)}
        <div
            class="band pack-lit"
            use:packPalette={band.palette}
            style:clip-path="path('{ribbonPath(band.ribbon)}')"
            style:--band-rim="path('{ribbonRimPath(band.ribbon)}')"
            style:--band-core="path('{ribbonCorePath(band.ribbon)}')"
            style:--band-fade={band.opacity}
        ></div>
    {/each}
</div>

<style>
    /* Spans the whole explorer so the bands can be positioned in the same
     * coordinates both panes were measured in, and so a band's own box *is*
     * the explorer's box, which is what lets it sample the field at offset zero
     * while the panes either side offset by their own position in it. */
    .projection {
        position: absolute;
        inset: 0;
        pointer-events: none;
    }
    /* The ribbon geometry is the clip; the paint is the pack's surface, and the
     * light rides over it exactly as it does on the card and the section. A
     * band's own box *is* the explorer's box, so it samples the field at offset
     * zero while the panes either side offset by their own position in it. */
    .band {
        position: absolute;
        inset: 0;
        background-position: 0 0;
        /* The pack's fade, carried along the band's length rather than over the
         * whole of it.
         *
         * A band is the middle of a pack, so it has to agree with a *different*
         * neighbour at each end: its card, which recedes and dims with the
         * rolodex, and its section, which does neither. One opacity for the
         * whole band can only match one of them. It matched the card, so every
         * band arrived at its section carrying the card's fade: a step in
         * alpha down the join, of a size that varied with how far that pack's
         * card sat from the focus line. Which reads as a seam that is somehow
         * different for every pack.
         *
         * So the fade runs from the card's own opacity where the band leaves it
         * to full where it arrives, and both ends match what they touch. The
         * band still sinks into the black with its card; it just stops taking
         * the section with it.
         *
         * A mask rather than `opacity` because the two ends need different
         * values. The band's box *is* the explorer's, so `--section-left` is
         * where the arrival end sits, the same number the section is measured
         * at. */
        mask-image: linear-gradient(
            to right,
            rgb(0 0 0 / var(--band-fade, 1)) var(--card-right, 0px),
            rgb(0 0 0 / 1) var(--section-left, 0px)
        );
    }
    /* The band's rim, which is a path and not a border: its edges are curves,
     * so there is no box whose padding could describe them. `ribbonRimPath` is
     * the same two cubics the fill is bounded by, taken a rim's width inward,
     * and open at both ends because both ends are interior to the pack. */
    .band::before {
        clip-path: var(--band-rim);
    }
    /* And the body the rim encloses, carrying the same share of the light the
     * card and the section hold their interiors to.
     *
     * A second layer because a clip is all or nothing: the two columns either
     * side say "full at the edge, `--pack-body-ramp` within" in a single mask,
     * which a box can do and a pair of cubics cannot. The regions are disjoint
     * by construction, so the two strengths meet rather than stack, and the
     * core sits a layer further back anyway, so a band pinched thin enough for
     * its rims to close over the core is the rim's colour and not a blend.
     *
     * The ramp itself is a plain mask here, with no ring to hold out of it, and
     * it needs no offset: a band's box *is* the explorer's, so the field and
     * the ramp both land at zero. This is the whole of the arithmetic the other
     * two columns need three layers to reproduce. */
    .band::after {
        content: '';
        z-index: -2;
        clip-path: var(--band-core);
        mask-image: var(--pack-body-ramp);
        mask-size: var(--field-w) var(--field-h);
        mask-position: 0 0;
        mask-repeat: no-repeat;
    }
</style>
