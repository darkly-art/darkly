<script lang="ts">
    /**
     * A pack's whole palette, at a glance.
     *
     * The card in miniature, so a painter recognises the same object in both
     * places — all four roles in the proportions the card spends them in,
     * rather than a dot of one colour out of four.
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
    /* The card in miniature: surface, with the same chroma and refraction
     * strands across the top. */
    .swatch {
        display: inline-block;
        flex: none;
        box-sizing: border-box;
        border-radius: var(--radius-sm);
        background: var(--pack-surface);
        box-shadow:
            inset 0 2px 0 0 var(--pack-chroma),
            inset 0 3px 0 0 var(--pack-refraction);
    }
</style>
