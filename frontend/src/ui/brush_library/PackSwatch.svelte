<script lang="ts">
    /**
     * A pack's whole palette, at a glance.
     *
     * The card in miniature, so a painter recognises the same object in both
     * places: the pack's surface under a wash of its vivid pair, in the
     * proportions the card spends them in, rather than a dot of one colour.
     *
     * Shared rather than modal-local on purpose: exporting is only the first
     * chooser to need it, and a create/edit dialog will want the same chip.
     */
    import { packPalette, type PackPalette } from '../../lib/packPalette';

    interface Props {
        palette: PackPalette;
        /** Edge length in px. */
        size?: number;
    }
    let { palette, size = 12 }: Props = $props();
</script>

<span
    class="swatch"
    use:packPalette={palette}
    style:width="{size}px"
    style:height="{size}px"
></span>

<style>
    /* The card in miniature: surface, lit the way a card under the column's
     * light is. Static, since a swatch does not scroll past anything.
     *
     * Surface beneath and light above, the same two layers the explorer's
     * columns are built from, so a swatch shows a pack the way the pack will
     * actually look, alpha and all, instead of being the one place its surface
     * is treated as opaque. Mixing the light into the surface instead would
     * count a translucent surface twice and leave the chip darker than the card
     * it stands for. */
    .swatch {
        display: inline-block;
        flex: none;
        box-sizing: border-box;
        border-radius: var(--radius-sm);
        background-color: var(--pack-surface);
        background-image: linear-gradient(
            180deg,
            color-mix(in srgb, var(--pack-refraction) 42%, transparent),
            transparent
        );
    }
</style>
