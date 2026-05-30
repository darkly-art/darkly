<script lang="ts">
    import { config } from '../config/store.svelte';
    import Modal from './Modal.svelte';
    import kritaSvg from '../assets/presets/krita.svg?raw';
    import gimpSvg from '../assets/presets/gimp.svg?raw';
    import photoshopSvg from '../assets/presets/photoshop.svg?raw';

    const logos: Record<string, string> = {
        krita: kritaSvg,
        gimp: gimpSvg,
        photoshop: photoshopSvg,
    };

    function pick(name: string) {
        // Just sets `app.baseSettings` in the user layer. The overlay
        // resolves live underneath any future user overrides — no file
        // copy, no "apply preset" step.
        config.setBase(name);
    }
</script>

<Modal bind:open={config.needsPresetChoice} size="sm" bare>
    <div class="preset-picker">
        <h2>Pick your starting hotkeys</h2>
        <p>You can change this anytime in Settings.</p>
        <div class="presets">
            {#each config.baseNames as name (name)}
                <button type="button" class="preset-btn" onclick={() => pick(name)}>
                    {#if logos[name.toLowerCase()]}
                        <span class="preset-logo" aria-hidden="true">{@html logos[name.toLowerCase()]}</span>
                    {/if}
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
        flex-direction: row;
        align-items: center;
        gap: 14px;
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        padding: 12px 16px;
        cursor: pointer;
        color: var(--text-muted);
        transition: border-color 0.15s, background 0.15s, color 0.15s;
    }

    .preset-btn:hover {
        background: var(--bg-active);
        border-color: var(--accent);
        color: var(--text);
    }

    .preset-logo {
        display: inline-flex;
        width: 24px;
        height: 24px;
        flex: 0 0 auto;
    }

    .preset-logo :global(svg) {
        width: 100%;
        height: 100%;
    }

    .preset-name {
        font-size: 15px;
        font-weight: 600;
        color: var(--text);
    }
</style>
