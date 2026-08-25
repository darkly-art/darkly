<script lang="ts">
    /**
     * Save the builder's active graph into the painter's library.
     *
     * Two verbs, and which are offered comes from `updateTarget`: saving over
     * the brush the graph came from, and saving as a new one. A brush that
     * ships with the app offers only the second — see `saveTarget.ts`.
     */
    import Modal from './Modal.svelte';
    import { app } from '../state/app.svelte';
    import { brushGraph } from '../state/brush_graph.svelte';
    import { brushLibrary } from '../state/brush_library.svelte';
    import { toast } from '../state/toast.svelte';
    import { newId } from '../lib/id';
    import { updateTarget } from './brush_builder/saveTarget';

    interface Props {
        open: boolean;
    }
    let { open = $bindable(false) }: Props = $props();

    const target = $derived(updateTarget(brushGraph.activeBrush, brushLibrary.brushes));

    let name = $state('');
    let busy = $state(false);

    // Seed the field from whatever is loaded each time the dialog opens, so it
    // never shows a name left over from a previous save.
    $effect(() => {
        if (open) name = brushGraph.activeBrush ?? '';
    });

    /** Save under `id`, then write it through so it survives a reload. The
     *  engine holds the library for this session only; `brushLibrary` is what
     *  makes it durable. */
    async function save(id: string) {
        const trimmed = name.trim();
        if (!trimmed || !app.engine || busy) return;
        busy = true;
        try {
            await app.engine.api.brushSave({ id, name: trimmed });
            await brushLibrary.persistBrush(id, trimmed);
            await brushLibrary.refresh();
            // The graph is now that brush, so the builder's title and every
            // tile keyed by the active name agree with the library.
            brushGraph.activeBrush = trimmed;
            toast.show('success', `Saved “${trimmed}”`);
            open = false;
        } catch (e) {
            toast.show('error', e instanceof Error ? e.message : String(e));
        } finally {
            busy = false;
        }
    }
</script>

<Modal bind:open title="Save to Library" size="sm">
    <div class="body">
        <label class="field">
            <span class="field-label">Name</span>
            <!-- svelte-ignore a11y_autofocus -->
            <input
                class="text-input"
                bind:value={name}
                autofocus
                placeholder="My Brush"
                onkeydown={e => {
                    if (e.key !== 'Enter') return;
                    e.preventDefault();
                    save(target ? target.id : newId('brush'));
                }}
            />
        </label>

        <div class="actions">
            <button class="btn" onclick={() => (open = false)}>Cancel</button>
            <div class="spacer"></div>
            <button
                class="btn"
                disabled={!name.trim() || busy}
                onclick={() => save(newId('brush'))}
            >Save as New</button>
            {#if target}
                <button
                    class="btn primary"
                    disabled={!name.trim() || busy}
                    onclick={() => save(target.id)}
                >Update “{target.name}”</button>
            {/if}
        </div>
    </div>
</Modal>

<style>
    .body {
        display: flex;
        flex-direction: column;
        gap: 14px;
    }
    .field {
        display: flex;
        flex-direction: column;
        gap: 5px;
    }
    .field-label {
        font-size: 11px;
        color: var(--text-muted);
    }
    .text-input {
        padding: 6px 8px;
        font-size: 12px;
        font-family: inherit;
        color: var(--text);
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        outline: none;
    }
    .text-input:focus {
        border-color: var(--accent);
    }
    .actions {
        display: flex;
        align-items: center;
        gap: 8px;
    }
    .spacer {
        flex: 1;
    }
    .btn {
        padding: 6px 12px;
        font-size: 12px;
        font-family: inherit;
        color: var(--text);
        background: var(--bg-hover);
        border: none;
        border-radius: 4px;
        cursor: pointer;
    }
    .btn:hover:not(:disabled) {
        background: var(--bg-active);
    }
    .btn:disabled {
        opacity: 0.5;
        cursor: default;
    }
    .btn.primary {
        color: var(--accent);
    }
</style>
