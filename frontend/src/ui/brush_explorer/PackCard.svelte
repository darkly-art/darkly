<script lang="ts">
    /**
     * One card on the rolodex. A pack, or a derived group like Recents.
     *
     * Reads `group.pack` and never `group.id`: the id is a list key, and asking
     * "which pack is this" to decide what to render is the consumer-side
     * classification the permission booleans exist to make unnecessary.
     */
    import Icon from '../../icons/Icon.svelte';
    import type { BrushGroup } from '../brush_library/grouping';

    interface Props {
        group: BrushGroup;
        /** The group currently under the list's focus line. */
        active: boolean;
        /** Rolodex transform, from `cardCurve`. */
        curve: { rotateX: number; scale: number; opacity: number };
        onSelect: () => void;
    }
    let { group, active, curve, onSelect }: Props = $props();
</script>

<button
    class="pack-card"
    class:active
    onclick={onSelect}
    style:transform="perspective(420px) rotateX({curve.rotateX}deg) scale({curve.scale})"
    style:opacity={curve.opacity}
    title={group.pack?.description || group.label}
>
    <span class="swatch" style:background={group.primary} style:border-color={group.secondary}></span>
    <Icon name={group.icon} class="card-icon" />
    <span class="label">{group.label}</span>
    <span class="count">{group.brushes.length}</span>
</button>

<style>
    .pack-card {
        display: flex;
        align-items: center;
        gap: 8px;
        width: 100%;
        padding: 10px 12px;
        font-family: inherit;
        font-size: 12px;
        color: var(--text-muted);
        text-align: left;
        background: var(--bg-hover);
        border: 1px solid transparent;
        border-radius: var(--radius-md);
        cursor: pointer;
        /* The curve is applied per card from `cardCurve`; keep the origin at the
         * card's own centre so the tilt reads as depth rather than sliding. */
        transform-origin: center center;
        will-change: transform, opacity;
        transition: background var(--transition-fast), color var(--transition-fast);
    }
    .pack-card:hover {
        background: var(--bg-active);
        color: var(--text);
    }
    .pack-card.active {
        background: var(--bg-active);
        border-color: var(--accent);
        color: var(--text);
    }
    .swatch {
        width: 10px;
        height: 10px;
        border-radius: 50%;
        border: 1.5px solid transparent;
        box-sizing: border-box;
        flex: none;
    }
    .pack-card :global(.card-icon) {
        font-size: 13px;
        flex: none;
    }
    .label {
        flex: 1;
        min-width: 0;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 0.4px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .count {
        flex: none;
        font-size: 10px;
        color: var(--text-dim);
        font-variant-numeric: tabular-nums;
    }
</style>
