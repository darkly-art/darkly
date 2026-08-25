<script lang="ts">
    import { app } from '../../state/app.svelte';
    import Modal from '../Modal.svelte';
    import EffectPreview from '../EffectPreview.svelte';
    import type { CaptureKind } from '../../lib/frameSource';
    import { actions } from '../../actions/registry';

    let { onclose }: { onclose: () => void } = $props();

    // Visible on mount; Modal owns backdrop/Escape/× dismissal and clears this
    // when closed, which we bridge back to the parent's `onclose` contract.
    let open = $state(true);
    $effect(() => {
        if (!open) onclose();
    });

    let voidTypes = $derived(app.entries?.('voids') ?? []);

    async function pick(vt: any) {
        if (!app.engine) return;
        // An image-sourced void has no empty state to add — it needs the user
        // to choose a file first — so hand it straight to the placement action
        // rather than creating a blank layer here. Keyed on the void's declared
        // source, so a future ingress is additive at this match.
        if (vt.source?.kind === 'image') {
            open = false;
            actions.dispatch('placeSmartObject', {});
            return;
        }
        // For MediaStream-backed voids (camera / screenshare), acquire the
        // MediaStream IN this click gesture, BEFORE the awaitable `add_void`
        // round-trip. `getDisplayMedia` requires transient user activation,
        // which would expire if we acquired only after awaiting add_void. If
        // the user cancels / denies, we still create the layer and record the
        // error so the properties panel can offer Resume. A `stream` void
        // (Blender) needs no gesture or permission — it connects over localhost
        // HTTP after the layer exists — so skip acquisition entirely.
        const captureKind: CaptureKind | undefined =
            vt.source?.kind === 'capture' ? vt.source.capture : undefined;
        let stream: MediaStream | undefined;
        let acquireError: unknown;
        if (captureKind === 'camera' || captureKind === 'display') {
            try {
                stream = await app.acquireMediaStream(captureKind);
            } catch (err) {
                acquireError = err;
            }
        }

        const defaults: Record<string, any> = {};
        for (const p of vt.params) {
            defaults[p.name] = p.default;
        }
        const id = await app.engine.api.addVoid({
            void_type: vt.type,
            params: defaults,
            anchor: app.activeLayerId,
        });
        if (id != null) {
            app.selectLayer(id);
            // Adding a stream-backed void via the picker is an explicit user
            // gesture — opt the new layer into this session's allow-list and
            // hand it the pre-acquired stream (or the acquire error). Reopening
            // a saved doc does NOT add to this set, which is why loaded
            // stream voids hold their saved frame until the user clicks Resume.
            if (captureKind) {
                app.markStreamVoidStarted(id);
                await app.startStreamSource(id, captureKind, stream, acquireError);
            }
        } else if (stream) {
            // Layer creation failed but we acquired a stream — release it so the
            // OS capture indicator doesn't linger.
            stream.getTracks().forEach((t) => t.stop());
        }
        app.requestFrame();
        open = false;
    }
</script>

<Modal bind:open title="Add Void" size="md">
    <div class="grid">
        {#each voidTypes as vt (vt.type)}
            <button class="card" onclick={() => pick(vt)}>
                <EffectPreview catalog="voids" entry={vt} />
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
