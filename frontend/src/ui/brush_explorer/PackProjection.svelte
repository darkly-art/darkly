<script lang="ts">
    /**
     * The band of colour thrown from the focused pack's card across to the
     * brushes it holds.
     *
     * The two panes show the same packs at different scales, and nothing in
     * either one says which card goes with which run of brushes. This does: it
     * leaves the card at card height, arrives at the section at section height,
     * and reshapes every frame the focus moves — so the wheel's compression is
     * something you watch happen rather than something you have to infer.
     *
     * Purely decorative, hence `aria-hidden` and no pointer events: the same
     * relation is already in the reading order, since the card names the pack
     * and the section follows it.
     */
    import { ribbonPath, type Ribbon } from './wheel';

    interface Props {
        /** Geometry in the overlay's own coordinates, or `null` when there is
         *  no focused pack (an empty search) or its section is off-screen. */
        ribbon: Ribbon | null;
        /** The focused pack's surface colour. */
        primary: string;
    }
    let { ribbon, primary }: Props = $props();
</script>

{#if ribbon}
    <svg class="projection" aria-hidden="true" style:color={primary}>
        <!-- Solid, and the same colour the card and the spine are painting.
             Anything less than opaque shows the black slab through the band and
             the eye reads that as a gap — the point of the ribbon is that the
             colour is *continuous* from the card to the brushes, so the fill
             cannot be a tint of the background it crosses. -->
        <path d={ribbonPath(ribbon)} fill="currentColor" />
    </svg>
{/if}

<style>
    /* Spans the whole explorer so the ribbon can be positioned in the same
     * coordinates both panes were measured in. It only ever paints the gutter
     * between them — the path starts at the card's edge and ends at the
     * section's — so covering the panes costs nothing. */
    .projection {
        position: absolute;
        inset: 0;
        width: 100%;
        height: 100%;
        pointer-events: none;
        overflow: visible;
    }
</style>
