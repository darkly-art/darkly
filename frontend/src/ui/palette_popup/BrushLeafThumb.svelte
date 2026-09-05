<script lang="ts">
    import { onDestroy, untrack } from 'svelte';
    import { app } from '../../state/app.svelte';
    import { BakedThumbnail } from '../../lib/bakedThumbnail.svelte';
    import Icon from '../../icons/Icon.svelte';

    interface Props {
        /** Library brush name: the engine's thumbnail lookup key. */
        name: string;
        /** Iconify fallback while the stroke bake is cold, and permanently
         *  for content-dependent brushes whose bake renders blank. */
        icon?: string | null;
    }
    let { name, icon = null }: Props = $props();

    const stroke = new BakedThumbnail(async () =>
        app.engine ? (await app.engine.api.brushThumbnail({ name })).bytes : undefined);

    $effect(() => {
        void app.engine;
        void name;
        untrack(() => stroke.request());
    });

    onDestroy(() => stroke.destroy());
</script>

{#if stroke.url}
    <img class="thumb" src={stroke.url} alt="" />
{:else}
    <span class="fallback"><Icon name={icon ?? 'fa6-solid:paintbrush'} /></span>
{/if}

<style>
    .thumb {
        display: block;
        width: 100%;
        height: 100%;
        object-fit: cover;
        border-radius: 3px;
    }
    .fallback {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 100%;
        height: 100%;
        color: var(--text-muted);
    }
</style>
