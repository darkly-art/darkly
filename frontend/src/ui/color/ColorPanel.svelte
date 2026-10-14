<script lang="ts">
    /**
     * The colour panel, docked: a tab strip over the wheel (with the swatches
     * picking which of the pair it edits) and the per-stroke jitter controls.
     * The wheel itself is the same component the ad-hoc popup hosts.
     */
    import { app } from '../../state/app.svelte';
    import type { Color } from '../../lib/color';
    import ColorWheel from './ColorWheel.svelte';
    import HexField from './HexField.svelte';
    import FgBgSwatches, { type SwatchTarget } from './FgBgSwatches.svelte';
    import JitterControls from './JitterControls.svelte';
    import Tabs from '../Tabs.svelte';

    const TABS = [
        { id: 'wheel', label: 'Wheel' },
        { id: 'jitter', label: 'Jitter' },
    ];

    let tab = $state('wheel');
    let target = $state<SwatchTarget>('foreground');
    let value = $derived(app[target]);

    function write(c: Color) {
        app[target] = c;
    }
</script>

<div class="color-panel">
    <Tabs tabs={TABS} active={tab} onselect={(id) => (tab = id)} />
    {#if tab === 'wheel'}
        <div class="header">
            <FgBgSwatches mode="select" active={target} onselect={(t) => (target = t)} />
            <div class="hex"><HexField {value} onchange={write} /></div>
        </div>
        <ColorWheel {value} oninput={write} onchange={write} size={220} />
    {:else}
        <JitterControls />
    {/if}
</div>

<style>
    .color-panel {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 12px;
        padding: 12px;
        overflow: auto;
        height: 100%;
        box-sizing: border-box;
    }
    .color-panel > :global(.tab-strip) {
        align-self: stretch;
    }
    .header {
        display: flex;
        align-items: center;
        gap: 12px;
    }
    .hex {
        width: 90px;
    }
</style>
