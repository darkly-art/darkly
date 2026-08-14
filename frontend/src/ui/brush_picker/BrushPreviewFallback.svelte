<!--
    Shown by the brush preview strips in the *dab* slot, in place of a
    baked dab thumbnail, for brushes whose graph samples existing canvas
    content (clone, blur, smudge, liquify). One stationary full-pressure
    sample has no motion for a displacement to reveal, so its bake shows
    nothing; the stroke slot beside this renders normally, over a field
    the brush is staged to transport. The icon comes from the node
    registration's `preview_staging`.
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
    /* Sized in `em` rather than as a percentage: the slot this fills is
     * square and its height comes from the strip's aspect ratio, and
     * percentage heights don't resolve against an aspect-ratio-derived
     * size (the svg would collapse to its intrinsic 1em). */
    .fallback :global(svg) {
        width: 2em;
        height: auto;
    }
</style>
