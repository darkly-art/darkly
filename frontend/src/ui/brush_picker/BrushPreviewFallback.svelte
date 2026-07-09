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
    /* Scale the icon with the strip height so it stays legible from the
     * 80px brush-bar strip up to the ~220px picker grid tiles. */
    .fallback :global(svg) {
        height: 55%;
        width: auto;
    }
</style>
