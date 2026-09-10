<script lang="ts">
    /**
     * The color wheel as an ad-hoc floating surface next to whatever opened
     * it. Fixed-positioned from the anchor's screen rect and clamped to the
     * viewport, so it escapes every clipping ancestor (panel bodies, modal
     * dialogs, scrolling preference lists) while staying in the trigger's
     * DOM subtree, which keeps it inside a `<dialog>`'s top layer.
     *
     * Dismissal is the `watchDismiss` rule: the host tags its trigger and this
     * surface tags itself with `data-keep-open={scope}`; a pointerdown anywhere
     * else closes it. Escape is window-level, since the popup never needs
     * focus to be open.
     */
    import type { Color } from '../../state/app.svelte';
    import { watchDismiss } from '../../lib/dismiss';
    import ColorWheel from './ColorWheel.svelte';
    import HexField from './HexField.svelte';

    let {
        value,
        oninput,
        onchange,
        onclose,
        scope,
        anchor,
    }: {
        value: Color;
        oninput: (c: Color) => void;
        onchange: (c: Color) => void;
        onclose: () => void;
        scope: string;
        anchor: HTMLElement;
    } = $props();

    const WHEEL_SIZE = 200;
    const MARGIN = 8;

    let surface = $state<HTMLDivElement | null>(null);
    let pos = $state({ x: 0, y: 0 });

    // Prefer the anchor's right side, top-aligned; fall back to its left, then
    // clamp into the viewport so a bottom-corner trigger still shows it whole.
    $effect(() => {
        if (!surface) return;
        const a = anchor.getBoundingClientRect();
        const w = surface.offsetWidth, h = surface.offsetHeight;
        let x = a.right + MARGIN;
        if (x + w > window.innerWidth - MARGIN) x = a.left - MARGIN - w;
        let y = a.top;
        x = Math.max(MARGIN, Math.min(x, window.innerWidth - MARGIN - w));
        y = Math.max(MARGIN, Math.min(y, window.innerHeight - MARGIN - h));
        pos = { x, y };
    });

    $effect(() => watchDismiss(scope, onclose));

    function onKeydown(e: KeyboardEvent) {
        if (e.key !== 'Escape') return;
        e.preventDefault();
        onclose();
    }

</script>

<svelte:window onkeydown={onKeydown} />

<div
    class="color-popup"
    bind:this={surface}
    data-keep-open={scope}
    style:left="{pos.x}px"
    style:top="{pos.y}px"
    role="dialog"
    aria-label="Color"
>
    <ColorWheel {value} {oninput} {onchange} size={WHEEL_SIZE} />
    <HexField {value} {onchange} />
</div>

<style>
    .color-popup {
        position: fixed;
        z-index: 1000;
        display: flex;
        flex-direction: column;
        gap: 8px;
        background: var(--bg-active);
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        padding: 8px;
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.5);
    }
</style>
