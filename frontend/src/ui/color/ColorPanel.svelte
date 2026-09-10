<script lang="ts">
    /**
     * The color wheel as a docked workspace panel. The swatches pick which of
     * the pair the wheel edits; the wheel itself is the same component the
     * ad-hoc popup hosts.
     */
    import { app, type Color } from '../../state/app.svelte';
    import ColorWheel from './ColorWheel.svelte';
    import HexField from './HexField.svelte';
    import FgBgSwatches, { type SwatchTarget } from './FgBgSwatches.svelte';

    let target = $state<SwatchTarget>('foreground');
    let value = $derived(app[target]);

    function write(c: Color) {
        app[target] = c;
    }

</script>

<div class="color-panel">
    <div class="header">
        <FgBgSwatches mode="select" active={target} onselect={(t) => (target = t)} />
        <div class="hex"><HexField {value} onchange={write} /></div>
    </div>
    <ColorWheel {value} oninput={write} onchange={write} size={220} />
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
    .header {
        display: flex;
        align-items: center;
        gap: 12px;
    }
    .hex {
        width: 90px;
    }
</style>
