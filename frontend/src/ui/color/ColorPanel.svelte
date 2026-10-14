<script lang="ts">
    /**
     * The color editor as a docked workspace panel. The swatches pick which of
     * the paint pair the editor edits; the editor itself is the same widget
     * the floating popup hosts.
     */
    import { app } from '../../state/app.svelte';
    import type { Color } from '../../lib/color';
    import ColorEditor from './ColorEditor.svelte';
    import FgBgSwatches, { type SwatchTarget } from './FgBgSwatches.svelte';

    let target = $state<SwatchTarget>('foreground');

    function write(c: Color) {
        app[target] = c;
    }
</script>

<div class="color-panel">
    <FgBgSwatches mode="select" active={target} onselect={(t) => (target = t)} />
    <ColorEditor value={app[target]} oninput={write} onchange={write} size={220} paint />
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
</style>
