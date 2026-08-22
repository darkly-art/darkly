<!--
  Pick a brush pack to export as a `.darkly-brush` file.

  A plain chooser, driven from the `exportBrushPack` action: import needs none
  (the OS picker is the chooser) but export has nothing to invoke it from while
  pack-management UI is deferred. When that push lands, exporting moves onto
  the pack row and this component is deleted.
-->
<script lang="ts">
    import Modal from './Modal.svelte';
    import Icon from '../icons/Icon.svelte';
    import { packExport } from '../state/packExport.svelte';
    import { brushLibrary } from '../state/brush_library.svelte';
    import { packIcon } from '../lib/packIcon';
    import { exportPack } from '../actions/pack_actions';

    async function choose(id: string) {
        packExport.open = false;
        await exportPack(id);
    }
</script>

<Modal bind:open={packExport.open} title="Export Brush Pack" size="sm">
    <div class="packs">
        {#each brushLibrary.packs as pack (pack.id)}
            <button class="pack" onclick={() => choose(pack.id)}>
                <span
                    class="swatch"
                    style:background={pack.primary}
                    style:border-color={pack.secondary}
                ></span>
                <Icon name={packIcon(pack.icon)} />
                <span class="name">{pack.name}</span>
                <span class="count">
                    {pack.members.length}
                    {pack.members.length === 1 ? 'brush' : 'brushes'}
                </span>
            </button>
        {/each}
    </div>
</Modal>

<style>
    .packs {
        display: flex;
        flex-direction: column;
        gap: 2px;
        min-width: 280px;
    }
    .pack {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 8px 10px;
        background: none;
        border: none;
        border-radius: 4px;
        color: var(--text);
        font-size: 13px;
        text-align: left;
        cursor: pointer;
    }
    .pack:hover {
        background: var(--bg-hover);
    }
    .swatch {
        width: 10px;
        height: 10px;
        border-radius: 50%;
        border: 1.5px solid transparent;
        box-sizing: border-box;
        flex: none;
    }
    .name {
        flex: 1;
    }
    .count {
        color: var(--text-muted);
        font-size: 11px;
    }
</style>
