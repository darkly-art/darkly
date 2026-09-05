<script lang="ts">
    import { palettePopup } from '../../state/palettePopup.svelte';
    import {
        layoutWheel,
        hitKey,
        HUB_R,
        type SectorGeom,
    } from './wheel_geometry';
    import BrushLeafThumb from './BrushLeafThumb.svelte';
    import Icon from '../../icons/Icon.svelte';

    const engaged = $derived(
        palettePopup.state.kind === 'engaged' ? palettePopup.state : null);
    const layout = $derived(
        engaged ? layoutWheel(palettePopup.tree, engaged.path) : []);
    const highlightKey = $derived(engaged ? hitKey(engaged.highlight) : '');

    /** Annular sector outline: outer arc forward, inner arc back. Angles
     *  increase clockwise on screen (+y down), hence sweep 1 then 0. */
    function sectorPath(s: SectorGeom, cx: number, cy: number): string {
        const a1 = s.a0 + s.span;
        const large = s.span > Math.PI ? 1 : 0;
        const p = (a: number, r: number) =>
            `${(cx + r * Math.cos(a)).toFixed(2)} ${(cy + r * Math.sin(a)).toFixed(2)}`;
        return `M ${p(s.a0, s.r1)} A ${s.r1} ${s.r1} 0 ${large} 1 ${p(a1, s.r1)}`
            + ` L ${p(a1, s.r0)} A ${s.r0} ${s.r0} 0 ${large} 0 ${p(s.a0, s.r0)} Z`;
    }

    function badgeStyle(s: SectorGeom, cx: number, cy: number): string {
        const mid = s.a0 + s.span / 2;
        const r = (s.r0 + s.r1) / 2;
        const x = cx + r * Math.cos(mid);
        const y = cy + r * Math.sin(mid);
        return `left: ${x.toFixed(1)}px; top: ${y.toFixed(1)}px;`;
    }

    const key = (s: SectorGeom) => `sector:${s.path.join('.')}`;

    // The gesture belongs to the pointer, but Escape / focus loss must still
    // bail out mid-thread. Window-level because the overlay never has focus.
    function onKeydown(e: KeyboardEvent) {
        if (!palettePopup.isOpen || e.key !== 'Escape') return;
        e.preventDefault();
        palettePopup.cancel();
    }
    function onBlur() {
        palettePopup.cancel();
    }
</script>

<svelte:window onkeydown={onKeydown} onblur={onBlur} />

{#if engaged}
    {@const cx = engaged.center.x}
    {@const cy = engaged.center.y}
    <!-- Non-modal dialog: not top-layer (only showModal() promotes), but its
         presence suppresses global hotkeys via the dialog[open] rule in
         config/hotkeys.svelte.ts. Display-only: input never touches it. -->
    <dialog open class="palette-popup" aria-label="Palette popup">
        <svg>
            {#each layout as s (key(s))}
                <path
                    class="sector"
                    class:swatch={s.node.visual.kind === 'swatch'}
                    class:highlighted={key(s) === highlightKey}
                    d={sectorPath(s, cx, cy)}
                    style:fill={s.node.visual.kind === 'swatch'
                        ? s.node.visual.color.slice(0, 7)
                        : undefined}
                />
            {/each}
            <circle
                class="hub"
                class:highlighted={highlightKey === 'hub'}
                cx={cx}
                cy={cy}
                r={HUB_R - 4}
            />
        </svg>
        {#each layout as s (key(s))}
            {#if s.node.visual.kind !== 'swatch'}
                <div class="badge" class:highlighted={key(s) === highlightKey}
                     style={badgeStyle(s, cx, cy)}>
                    <div class="glyph">
                        {#if s.node.visual.kind === 'brush'}
                            <BrushLeafThumb name={s.node.visual.name} icon={s.node.visual.icon} />
                        {:else}
                            <Icon name={s.node.visual.icon} />
                        {/if}
                    </div>
                    <div class="label">{s.node.label}</div>
                </div>
            {/if}
        {/each}
    </dialog>
{/if}

<style>
    .palette-popup {
        position: fixed;
        inset: 0;
        width: 100%;
        height: 100%;
        margin: 0;
        padding: 0;
        border: none;
        background: transparent;
        z-index: 1500;
        pointer-events: none;
        overflow: hidden;
    }
    svg {
        width: 100%;
        height: 100%;
        display: block;
    }
    .sector {
        fill: color-mix(in srgb, var(--bg-raised) 88%, transparent);
        stroke: var(--bg);
        stroke-width: 2;
        animation: pop 90ms ease-out;
    }
    .sector.highlighted {
        fill: var(--bg-active);
    }
    .sector.swatch {
        stroke: var(--bg);
    }
    .sector.swatch.highlighted {
        stroke: var(--text);
    }
    .hub {
        fill: color-mix(in srgb, var(--bg-raised) 92%, transparent);
        stroke: var(--text-dim);
        stroke-width: 1;
    }
    .hub.highlighted {
        stroke: var(--text-muted);
    }
    .badge {
        position: absolute;
        transform: translate(-50%, -50%);
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 2px;
        width: 60px;
        color: var(--text);
        animation: pop 90ms ease-out;
    }
    .badge.highlighted {
        color: var(--text);
    }
    .glyph {
        width: 44px;
        height: 26px;
        display: flex;
        align-items: center;
        justify-content: center;
        font-size: 15px;
    }
    .label {
        max-width: 60px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        font-size: 10px;
        line-height: 1.2;
        text-align: center;
    }
    @keyframes pop {
        from { opacity: 0; }
        to { opacity: 1; }
    }
</style>
