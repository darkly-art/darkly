<script lang="ts">
    /**
     * The bands joining each pack's card to the brushes it holds — one per pack
     * currently on screen.
     *
     * **One per pack, not one for the focused pack.** A single band would have
     * to change hands whenever the focus did, and the two cards it would move
     * between are a whole card apart at that moment: every pack boundary would
     * flick the band across that gap. Drawing them all makes the transition
     * something that cannot happen — a pack scrolling out has its band shrink
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
     * keeping the two in step by hand — the duplication that guarantees a
     * visible seam the moment they drift. A div takes the same declaration
     * verbatim: same image, same size, same origin, so the three columns are one
     * surface and the joins need nothing done to them.
     *
     * Purely decorative, hence `aria-hidden` and no pointer events: the same
     * relation is already in the reading order, since the card names the pack
     * and the section follows it.
     */
    import { packPalette } from '../../lib/packPalette';
    import { ribbonPath, ribbonRimPath, type PackBand } from './wheel';

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
            style:opacity={band.opacity}
        ></div>
    {/each}
</div>

<style>
    /* Spans the whole explorer so the bands can be positioned in the same
     * coordinates both panes were measured in — and so a band's own box *is*
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
    }
    /* The band's rim, which is a path and not a border — its edges are curves,
     * so there is no box whose padding could describe them. `ribbonRimPath` is
     * the same two cubics the fill is bounded by, taken a rim's width inward,
     * and open at both ends because both ends are interior to the pack. */
    .band::before {
        clip-path: var(--band-rim);
    }
</style>
