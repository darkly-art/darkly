<!--
    Fallback shown by the brush preview strips in place of the baked
    dab/stroke thumbnails, for brushes whose graph samples existing
    canvas content (clone, blur, smudge, liquify) — stroking the flat
    preview background renders blank, so a bake would show nothing.
    The icon comes from the node registration's `preview_fallback_icon`.
-->
<script lang="ts">
    import Icon from '../../icons/Icon.svelte';

    interface Props {
        /** Full Iconify name, e.g. "fa6-solid:clone". */
        icon: string;
    }
    let { icon }: Props = $props();
</script>

<div class="fallback">
    <Icon name={icon} inline={false} />
</div>

<style>
    .fallback {
        width: 100%;
        height: 100%;
        display: flex;
        align-items: center;
        justify-content: center;
        color: var(--text-muted);
    }
    /* Scale the icon with the strip. Sized via a *width* percentage:
     * the strip's height comes from its 11:3 aspect-ratio, and
     * percentage heights don't resolve against an aspect-ratio-derived
     * size (the svg would collapse to its intrinsic 1em) — widths
     * always resolve. 22% of the width ≈ 80% of the strip height. */
    .fallback :global(svg) {
        width: 3em;
        height: auto;
    }
</style>
