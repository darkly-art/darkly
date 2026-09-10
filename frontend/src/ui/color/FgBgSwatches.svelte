<script lang="ts" module>
    export type SwatchTarget = 'foreground' | 'background';
</script>

<script lang="ts">
    /**
     * The foreground/background swatch pair in its Photoshop layout, which is
     * also what GIMP's `GimpFgBgEditor` (app/widgets/gimpfgbgeditor.c) and
     * Krita's `KoDualColorButton` (libs/ui/widgets/KoDualColorButton.cpp)
     * draw: the foreground square over the background square, a swap glyph in
     * the free top-right corner and a reset-to-default glyph in the free
     * bottom-left one. Four real buttons in one box, so each has its own
     * tooltip, focus, and keep-open scope.
     *
     * In `popup` mode a swatch click opens the color wheel for that swatch. In
     * `select` mode (the docked color panel, which shows its own wheel) a click
     * only reports which swatch the host should edit.
     */
    import { app, type Color } from '../../state/app.svelte';
    import { tooltipForAction } from '../../config/store.svelte';
    import Icon from '../../icons/Icon.svelte';
    import ColorPopup from './ColorPopup.svelte';

    let {
        mode,
        active,
        onselect,
    }: {
        mode: 'popup' | 'select';
        active?: SwatchTarget;
        onselect?: (target: SwatchTarget) => void;
    } = $props();

    const SCOPE = 'fg-bg-color';

    let open = $state<SwatchTarget | null>(null);
    let fgButton = $state<HTMLButtonElement>();
    let bgButton = $state<HTMLButtonElement>();

    function css(c: Color): string {
        return `rgb(${c.r}, ${c.g}, ${c.b})`;
    }

    function pick(target: SwatchTarget) {
        if (mode === 'select') onselect?.(target);
        else open = open === target ? null : target;
    }

    function write(target: SwatchTarget, c: Color) {
        app[target] = c;
    }
</script>

<div class="swatches">
    <button
        class="swatch bg"
        class:active={active === 'background'}
        bind:this={bgButton}
        data-keep-open={SCOPE}
        style:background={css(app.background)}
        onclick={() => pick('background')}
        title="Background color"
        aria-label="Background color"
    ></button>
    <button
        class="swatch fg"
        class:active={active === 'foreground'}
        bind:this={fgButton}
        data-keep-open={SCOPE}
        style:background={css(app.foreground)}
        onclick={() => pick('foreground')}
        title="Foreground color"
        aria-label="Foreground color"
    ></button>
    <button class="glyph swap" onclick={() => app.swapColors()} title={tooltipForAction('Swap colors', 'swapColors')}>
        <Icon name="fa6-solid:arrow-right-arrow-left" />
    </button>
    <button class="glyph reset" onclick={() => app.resetColors()} title={tooltipForAction('Reset colors', 'resetColors')}>
        <span class="mini bg"></span>
        <span class="mini fg"></span>
    </button>
</div>

{#if open}
    <ColorPopup
        value={app[open]}
        oninput={(c) => open && write(open, c)}
        onchange={(c) => open && write(open, c)}
        onclose={() => (open = null)}
        scope={SCOPE}
        anchor={(open === 'foreground' ? fgButton : bgButton)!}
    />
{/if}

<style>
    .swatches {
        position: relative;
        width: 32px;
        height: 32px;
        flex: none;
    }
    .swatches button {
        position: absolute;
        padding: 0;
        border: none;
        cursor: pointer;
    }
    .swatch {
        width: 20px;
        height: 20px;
        border-radius: 3px;
        box-shadow: 0 0 0 1px var(--text-dim);
    }
    .swatch.fg {
        top: 0;
        left: 0;
        z-index: 1;
    }
    .swatch.bg {
        right: 0;
        bottom: 0;
    }
    .swatch.active {
        box-shadow: 0 0 0 2px var(--accent);
        z-index: 2;
    }
    .glyph {
        width: 11px;
        height: 11px;
        background: none;
        color: var(--text-muted);
        font-size: 9px;
        display: flex;
        align-items: center;
        justify-content: center;
    }
    .glyph:hover {
        color: var(--text);
    }
    .glyph :global(svg) {
        width: 1em;
        height: 1em;
        fill: currentColor;
    }
    .swap {
        top: 0;
        right: 0;
        transform: rotate(-45deg);
    }
    .reset {
        bottom: 0;
        left: 0;
    }
    .mini {
        position: absolute;
        width: 6px;
        height: 6px;
        border-radius: 1px;
        box-shadow: 0 0 0 1px var(--text-dim);
    }
    .mini.fg {
        top: 0;
        left: 0;
        background: #000;
        z-index: 1;
    }
    .mini.bg {
        right: 0;
        bottom: 0;
        background: #fff;
    }
</style>
