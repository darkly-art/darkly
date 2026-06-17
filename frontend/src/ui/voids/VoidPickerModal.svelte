<script lang="ts">
    import { app } from '../../state/app.svelte';
    import Modal from '../Modal.svelte';

    let { onclose }: { onclose: () => void } = $props();

    // Visible on mount; Modal owns backdrop/Escape/× dismissal and clears this
    // when closed, which we bridge back to the parent's `onclose` contract.
    let open = $state(true);
    $effect(() => {
        if (!open) onclose();
    });

    let voidTypes = $state<any[]>([]);

    $effect(() => {
        const engine = app.engine;
        if (!engine) return;
        (async () => {
            try {
                const list = await engine.send('void_types');
                voidTypes = Array.isArray(list) ? list : [];
            } catch {
                voidTypes = [];
            }
        })();
    });

    async function pick(vt: any) {
        if (!app.engine) return;
        const defaults: Record<string, any> = {};
        for (const p of vt.params) {
            defaults[p.name] = p.default;
        }
        const id = (await app.engine.send('add_void', {
            void_type: vt.type,
            params: defaults,
            anchor: app.activeLayerId ?? -1,
        })).id;
        if (id >= 0) {
            app.selectLayer(id);
            // Adding a camera void via the picker is an explicit user
            // gesture — opt the new layer into this session's camera
            // allow-list so the reconciler spins up the MediaStream.
            // Reopening a saved doc does NOT add to this set, which is
            // why loaded camera voids hold their saved frame until the
            // user clicks Resume in VoidProperties.
            if (vt.type === 'camera') {
                app.markCameraVoidStarted(id);
            }
        }
        app.requestFrame();
        open = false;
    }
</script>

<Modal bind:open title="Add Void" size="md">
    <div class="grid">
        {#each voidTypes as vt (vt.type)}
            <button class="card" onclick={() => pick(vt)}>
                <div class="preview"></div>
                <span class="card-name">{vt.displayName}</span>
            </button>
        {/each}
        {#if voidTypes.length === 0}
            <div class="empty">No void types available</div>
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

    .preview {
        aspect-ratio: 16 / 9;
        background: var(--bg);
        border-radius: var(--radius-sm);
        background-image: linear-gradient(
            45deg,
            color-mix(in srgb, var(--accent) 20%, transparent) 0%,
            transparent 70%
        );
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
