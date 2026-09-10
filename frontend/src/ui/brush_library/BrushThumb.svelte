<!--
    One baked brush-thumbnail slot: the engine's PNG for a library brush, or
    a glyph when this slot has no image to show.

    The single owner of that pairing. Everywhere a miniature preview of a
    *library* brush appears (the picker's tiles, the palette popup's chips) it
    is one or two of these inside a `.brush-thumbs` envelope, so a slot's
    fetching, its theme invalidation and its fallback are decided once.
-->
<script lang="ts">
    import { onDestroy, untrack } from 'svelte';
    import { app } from '../../state/app.svelte';
    import { theme } from '../../state/theme.svelte';
    import { BakedThumbnail } from '../../lib/bakedThumbnail.svelte';
    import BrushPreviewFallback from './BrushPreviewFallback.svelte';
    import type { EngineApi } from '../../engine/protocol_gen';

    /**
     * What each kind of slot bakes, and whether a glyph replaces its bake.
     *
     * A content-dependent brush (clone, blur, smudge, liquify) declares an
     * icon because one stationary full-pressure sample has nothing to
     * displace, so its *dab* bake renders blank and must not be fired at all.
     * Its *stroke* bakes normally, over a striped backdrop the brush is
     * staged to transport, so a stroke slot always fetches and falls back to
     * the glyph only while cold. That difference belongs to the slot kind
     * rather than to each call site, which is where it used to live.
     */
    const KINDS = {
        stroke: {
            bake: (api: EngineApi, name: string) => api.brushThumbnail({ name }),
            iconSuppressesBake: false,
        },
        dab: {
            bake: (api: EngineApi, name: string) => api.brushDabThumbnail({ name }),
            iconSuppressesBake: true,
        },
    } as const;

    interface Props {
        /** Library brush name: the engine's thumbnail lookup key. */
        name: string;
        /** Which bake this slot shows. */
        kind?: keyof typeof KINDS;
        /** Glyph shown when this slot has no image (see `KINDS`). */
        icon?: string | null;
    }
    let { name, kind = 'stroke', icon = null }: Props = $props();

    const suppressed = $derived(icon !== null && KINDS[kind].iconSuppressesBake);

    const baked = new BakedThumbnail(async () =>
        app.engine && !suppressed
            ? (await KINDS[kind].bake(app.engine.api, name)).bytes
            : undefined);

    // The wasm handle arriving, a theme swap (which invalidates every baked
    // PNG through `set_preview_theme`), and the brush or slot kind changing
    // all call for fresh bytes.
    $effect(() => {
        void app.engine;
        void theme.current;
        void name;
        void kind;
        void suppressed;
        untrack(() => baked.request());
    });

    onDestroy(() => baked.destroy());
</script>

{#if baked.url}
    <img src={baked.url} alt="" />
{:else if icon}
    <BrushPreviewFallback {icon} />
{/if}
