<script lang="ts">
    import { app } from '../../state/app.svelte';
    import EffectPreview from '../EffectPreview.svelte';
    import Modal from '../Modal.svelte';

    let { onclose }: { onclose: () => void } = $props();

    // Visible on mount; Modal owns backdrop/Escape/× dismissal and clears this
    // when closed, which we bridge back to the parent's `onclose` contract.
    let open = $state(true);
    $effect(() => {
        if (!open) onclose();
    });

    let veilTypes = $state<any[]>([]);

    $effect(() => {
        const engine = app.engine;
        if (!engine) return;
        (async () => {
            try {
                const list = await engine.api.veilTypes();
                veilTypes = Array.isArray(list) ? list : [];
            } catch {
                veilTypes = [];
            }
        })();
    });

    async function pick(vt: any) {
        if (!app.engine) return;
        const defaults: Record<string, any> = {};
        for (const p of vt.params) {
            defaults[p.name] = p.default;
        }
        await app.addVeil(vt.type, defaults);
        // Select the newly added veil (added at end of list).
        app.selectVeil(app.veilList.length - 1);
        open = false;
    }
</script>

<Modal bind:open title="Add Veil" size="md">
    <div class="grid">
        {#each veilTypes as vt (vt.type)}
            <button class="card" onclick={() => pick(vt)}>
                <EffectPreview kind="veil" type={vt.type} />
                <span class="card-name">{vt.displayName}</span>
            </button>
        {/each}
        {#if veilTypes.length === 0}
            <div class="empty">No veil types available</div>
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
