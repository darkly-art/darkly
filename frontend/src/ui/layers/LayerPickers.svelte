<script lang="ts">
    import { app } from '../../state/app.svelte';
    import { layerPicker } from '../../state/layerPicker.svelte';
    import VeilPickerModal from '../veils/VeilPickerModal.svelte';
    import VoidPickerModal from '../voids/VoidPickerModal.svelte';
    import FilterPickerModal from '../filters/FilterPickerModal.svelte';

    // The pickers add layers/veils by type, so the panel's view of both has to
    // be re-read once one closes. They're mounted at the app root — reachable
    // from the palette and menu bar, not just the layer panel — so the refresh
    // is theirs to do rather than the panel's.
    function close() {
        layerPicker.kind = null;
        if (!app.engine) return;
        app.refreshLayerTree();
        app.refreshVeilList();
        app.requestFrame();
    }
</script>

{#if layerPicker.kind === 'veil'}
    <VeilPickerModal onclose={close} />
{:else if layerPicker.kind === 'void'}
    <VoidPickerModal onclose={close} />
{:else if layerPicker.kind === 'filter'}
    <FilterPickerModal onclose={close} />
{/if}
