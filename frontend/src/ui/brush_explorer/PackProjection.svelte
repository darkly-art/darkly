<script lang="ts">
    /**
     * The bands of colour thrown from the pack cards across to the brushes they
     * hold — one per pack currently on screen.
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
     * A band is the *middle* of the pack rather than a decoration between two
     * halves of it: the card, this and the section are one shape, so the band
     * carries the same fill and the same edge strands they do, and its two
     * vertical ends are left unstroked because they are interior to that shape.
     *
     * Purely decorative, hence `aria-hidden` and no pointer events: the same
     * relation is already in the reading order, since the card names the pack
     * and the section follows it.
     */
    import { ribbonPath, type PackBand } from './wheel';

    interface Props {
        bands: PackBand[];
    }
    let { bands }: Props = $props();

</script>

<svg class="projection" aria-hidden="true">
    {#each bands as band (band.id)}
        <!-- Grouped so the rolodex fade applies to the band as one object.
             `style:` rather than presentation attributes throughout, because a
             derived group's palette holds `var(--…)` references and those do
             not parse in an SVG attribute. As CSS properties both those and
             literal hex work, so there is no branch. -->
        <g opacity={band.opacity}>
            <path d={ribbonPath(band.ribbon)} style:fill={band.palette.surface} />
        </g>
    {/each}
</svg>

<style>
    /* Spans the whole explorer so the bands can be positioned in the same
     * coordinates both panes were measured in. They only ever paint the gutter
     * between them — each path starts at a card's edge and ends at a section's
     * — so covering the panes costs nothing. */
    .projection {
        position: absolute;
        inset: 0;
        width: 100%;
        height: 100%;
        pointer-events: none;
        overflow: visible;
    }
</style>
