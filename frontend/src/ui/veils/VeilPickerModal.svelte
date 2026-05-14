<script lang="ts">
    import { app } from '../../state/app.svelte';

    let { onclose }: { onclose: () => void } = $props();

    let veilTypes = $state<any[]>([]);

    $effect(() => {
        if (app.handle) {
            try {
                veilTypes = JSON.parse(app.handle.veil_types());
            } catch {
                veilTypes = [];
            }
        }
    });

    function pick(vt: any) {
        if (!app.handle) return;
        const defaults: Record<string, any> = {};
        for (const p of vt.params) {
            defaults[p.name] = p.default;
        }
        app.handle.add_veil(vt.type, defaults);
        app.refreshVeilList();
        // Select the newly added veil (added at end of list).
        app.selectVeil(app.veilList.length - 1);
        app.requestFrame();
        onclose();
    }

    function onBackdropClick(e: MouseEvent) {
        if (e.target === e.currentTarget) onclose();
    }

    function onKeyDown(e: KeyboardEvent) {
        if (e.key === 'Escape') onclose();
    }
</script>

<svelte:window onkeydown={onKeyDown} />

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="backdrop" onclick={onBackdropClick}>
    <div class="modal" role="dialog" aria-label="Pick a veil">
        <div class="modal-header">
            <span class="modal-title">Add Veil</span>
            <button class="close-btn" onclick={onclose} title="Close">
                <i class="fa-solid fa-xmark"></i>
            </button>
        </div>
        <div class="grid">
            {#each veilTypes as vt (vt.type)}
                <button class="card" onclick={() => pick(vt)}>
                    <!-- TODO: replace with <video> preview when assets land -->
                    <div class="preview"></div>
                    <span class="card-name">{vt.displayName}</span>
                </button>
            {/each}
            {#if veilTypes.length === 0}
                <div class="empty">No veil types available</div>
            {/if}
        </div>
    </div>
</div>

<style>
    .backdrop {
        position: fixed;
        inset: 0;
        background: rgba(0, 0, 0, 0.5);
        display: flex;
        align-items: center;
        justify-content: center;
        z-index: 1000;
    }

    .modal {
        background: var(--bg-surface, var(--bg));
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-md);
        box-shadow: 0 8px 32px rgba(0, 0, 0, 0.5);
        width: min(520px, 90vw);
        max-height: 80vh;
        display: flex;
        flex-direction: column;
        overflow: hidden;
    }

    .modal-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 10px 14px;
        background: var(--bg-hover);
    }

    .modal-title {
        font-size: 12px;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 1px;
        color: var(--text);
    }

    .close-btn {
        width: 24px;
        height: 24px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: none;
        border: none;
        color: var(--text-muted);
        cursor: pointer;
        border-radius: var(--radius-sm);
        font-size: 13px;
    }
    .close-btn:hover {
        background: var(--bg-active);
        color: var(--text);
    }

    .grid {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
        gap: 10px;
        padding: 14px;
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
