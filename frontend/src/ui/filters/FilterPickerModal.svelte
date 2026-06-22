<script lang="ts">
    import { app } from '../../state/app.svelte';
    import Modal from '../Modal.svelte';
    import Icon from '../../icons/Icon.svelte';

    let { onclose }: { onclose: () => void } = $props();

    // Visible on mount; Modal owns backdrop/Escape/× dismissal and clears this
    // when closed, which we bridge back to the parent's `onclose` contract.
    let open = $state(true);
    $effect(() => {
        if (!open) onclose();
    });

    // The filter pipeline registry is fetched once at startup into
    // `app.filterTypes` (same list that drives the destructive Colors menu),
    // so a new filter in the Rust core surfaces here with no frontend edit.
    async function pick(ft: { type: string; displayName: string }) {
        if (!app.engine) return;
        const { id } = await app.engine.send('add_filter_layer', {
            pipeline: ft.type,
            params: {},
            anchor: app.activeLayerId ?? -1,
        });
        if (id >= 0) app.selectLayer(id);
        app.requestFrame();
        open = false;
    }
</script>

<Modal bind:open title="Add Filter Layer" size="md">
    <div class="grid">
        {#each app.filterTypes ?? [] as ft (ft.type)}
            <button class="card" onclick={() => pick(ft)}>
                <div class="preview preview-icon">
                    <Icon name="fa6-solid:circle-half-stroke" />
                </div>
                <span class="card-name">{ft.displayName}</span>
            </button>
        {/each}
        {#if (app.filterTypes ?? []).length === 0}
            <div class="empty">No filter types available</div>
        {/if}
    </div>
</Modal>

<style>
    .grid {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
        gap: 10px;
        overflow-y: auto;
    }

    .card {
        display: flex;
        flex-direction: column;
        gap: 6px;
        padding: 8px;
        background: var(--bg-hover);
        border: 1px solid transparent;
        border-radius: var(--radius-md);
        color: var(--text);
        cursor: pointer;
        transition: background var(--transition-fast), border-color var(--transition-fast);
    }
    .card:hover {
        background: var(--bg-active);
        border-color: var(--accent);
    }

    .preview-icon {
        aspect-ratio: 16 / 9;
        background: var(--bg);
        border-radius: var(--radius-sm);
        display: flex;
        align-items: center;
        justify-content: center;
        font-size: 32px;
        color: var(--text-dim);
    }

    .card-name {
        font-size: 12px;
        text-align: center;
        text-transform: capitalize;
    }

    .empty {
        grid-column: 1 / -1;
        text-align: center;
        color: var(--text-dim);
        font-size: 12px;
        padding: 20px;
    }
</style>
