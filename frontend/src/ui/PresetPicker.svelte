<script lang="ts">
    import { config } from '../config/store.svelte';
    import { actions } from '../actions/registry';
    import { registerHotkeys } from '../config/hotkeys.svelte';

    function pick(name: string) {
        config.applyPreset(name);
        config.needsPresetChoice = false;
        // Re-register hotkeys only if actions are already registered
        // (initEditor may not have run yet — it will call registerHotkeys itself)
        if (actions.ids().length > 0) {
            registerHotkeys();
        }
    }
</script>

{#if config.needsPresetChoice}
    <div class="backdrop">
        <div class="modal">
            <h2>Choose your keybinding preset</h2>
            <p>This sets keyboard shortcuts and modifier behaviors. You can change it later in settings.</p>
            <div class="presets">
                {#each config.presets as preset}
                    <button class="preset-btn" onclick={() => pick(preset.name)}>
                        <span class="preset-name">{preset.name}</span>
                        <span class="preset-desc">{preset.description}</span>
                    </button>
                {/each}
            </div>
        </div>
    </div>
{/if}

<style>
    .backdrop {
        position: fixed;
        inset: 0;
        background: rgba(0, 0, 0, 0.7);
        display: flex;
        align-items: center;
        justify-content: center;
        z-index: 2000;
    }

    .modal {
        background: var(--bg-active);
        border: 1px solid var(--bg-hover);
        border-radius: 8px;
        padding: 32px;
        max-width: 400px;
        width: 90%;
        text-align: center;
    }

    h2 {
        margin: 0 0 8px;
        font-size: 18px;
        font-weight: 600;
        color: var(--text);
    }

    p {
        margin: 0 0 24px;
        font-size: 13px;
        color: var(--text-muted);
    }

    .presets {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .preset-btn {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 2px;
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        padding: 14px 16px;
        cursor: pointer;
        transition: border-color 0.15s, background 0.15s;
    }

    .preset-btn:hover {
        background: var(--bg-active);
        border-color: var(--accent);
    }

    .preset-name {
        font-size: 15px;
        font-weight: 600;
        color: var(--text);
    }

    .preset-desc {
        font-size: 12px;
        color: var(--text-muted);
    }
</style>
