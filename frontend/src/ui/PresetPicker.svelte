<script lang="ts">
    import { config } from '../config/store.svelte';
    import Modal from './Modal.svelte';

    function pick(name: string) {
        // Just sets `app.baseSettings` in the user layer. The overlay
        // resolves live underneath any future user overrides — no file
        // copy, no "apply preset" step.
        config.setBase(name);
    }
</script>

<Modal bind:open={config.needsPresetChoice} size="sm" bare>
    <div class="preset-picker">
        <h2>Choose your starting keybindings</h2>
        <p>Pick a familiar layout to seed your settings. Each option is just a
            base layer — you can rebind anything, and you can switch base
            layouts later from Settings without losing your customizations.</p>
        <div class="presets">
            {#each config.baseNames as name (name)}
                <button type="button" class="preset-btn" onclick={() => pick(name)}>
                    <span class="preset-name">{name}</span>
                </button>
            {/each}
        </div>
    </div>
</Modal>

<style>
    .preset-picker {
        padding: 32px;
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
</style>
