<script lang="ts">
    import { app } from '../../state/app.svelte';
    import Modal from '../Modal.svelte';
    import EffectPreview from '../EffectPreview.svelte';
    import Icon from '../../icons/Icon.svelte';
    import { voidShowsPreview } from '../preview_frames';
    import type { CaptureKind } from '../../lib/mediaStreamSource';

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
        // For stream-backed voids (camera / screenshare), acquire the
        // MediaStream IN this click gesture, BEFORE the awaitable `add_void`
        // round-trip. `getDisplayMedia` requires transient user activation,
        // which would expire if we acquired only after awaiting add_void. If
        // the user cancels / denies, we still create the layer and record the
        // error so the properties panel can offer Resume.
        const captureKind: CaptureKind | undefined = vt.captureKind;
        let stream: MediaStream | undefined;
        let acquireError: unknown;
        if (captureKind) {
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
        const id = (await app.engine.send('add_void', {
            void_type: vt.type,
            params: defaults,
            anchor: app.activeLayerId,
        })).id;
        if (id != null) {
            app.selectLayer(id);
            // Adding a stream-backed void via the picker is an explicit user
            // gesture — opt the new layer into this session's allow-list and
            // hand it the pre-acquired stream (or the acquire error). Reopening
            // a saved doc does NOT add to this set, which is why loaded
            // stream voids hold their saved frame until the user clicks Resume.
            if (captureKind) {
                app.markMediaStreamVoidStarted(id);
                await app.startMediaStreamVoid(id, captureKind, stream, acquireError);
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
                {#if voidShowsPreview(vt)}
                    <EffectPreview kind="void" type={vt.type} />
                {:else}
                    <div class="preview preview-icon">
                        <Icon name={vt.icon} />
                    </div>
                {/if}
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

    /* Fallback box shown for voids without a rendered preview — a centered
       iconify glyph declared by the void's registration. */
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
