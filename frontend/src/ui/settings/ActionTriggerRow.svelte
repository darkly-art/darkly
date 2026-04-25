<script lang="ts">
    import { config, formatHotkey } from '../../config/store.svelte';
    import type { ActionRegistration } from '../../actions/registry';
    import HotkeyCapture from './widgets/HotkeyCapture.svelte';
    import MouseChordCapture from './widgets/MouseChordCapture.svelte';

    type Props = { action: ActionRegistration };
    let { action }: Props = $props();

    const hotkeyKey = $derived(`hotkeys.${action.id}`);
    const mouseKey = $derived(`mouseclicks.${action.id}`);

    const hotkeyValue = $derived.by(() => {
        const v = config.get(hotkeyKey);
        return typeof v === 'string' ? v : (action.defaultHotkey ?? '');
    });

    const mouseValue = $derived.by(() => {
        const v = config.get(mouseKey);
        return typeof v === 'string' ? v : (action.defaultMouseClick ?? '');
    });

    const hotkeyOverridden = $derived(config.hasOverride(hotkeyKey));
    const mouseOverridden = $derived(config.hasOverride(mouseKey));

    function setHotkey(v: string) { config.set(hotkeyKey, v); }
    function setMouse(v: string) { config.set(mouseKey, v); }

    function resetHotkey() { config.resetKey(hotkeyKey); }
    function resetMouse() { config.resetKey(mouseKey); }

    // Description doubles as the help row when present.
    const desc = $derived(action.description ?? null);
</script>

<div class="row">
    <div class="label-col">
        <div class="label">{action.displayName}</div>
        {#if desc}<div class="desc">{desc}</div>{/if}
    </div>

    <div class="trigger-col">
        <div class="trigger-cell">
            <HotkeyCapture
                prefKey={hotkeyKey}
                value={hotkeyValue}
                onchange={setHotkey}
            />
            <button
                type="button"
                class="reset"
                class:visible={hotkeyOverridden}
                disabled={!hotkeyOverridden}
                onclick={resetHotkey}
                title="Reset to default ({formatHotkey(action.defaultHotkey ?? '') ?? 'unbound'})"
            >
                <i class="fa-solid fa-rotate-left"></i>
            </button>
        </div>

        <div class="trigger-cell">
            <MouseChordCapture
                {action}
                value={mouseValue}
                onchange={setMouse}
            />
            <button
                type="button"
                class="reset"
                class:visible={mouseOverridden}
                disabled={!mouseOverridden}
                onclick={resetMouse}
                title="Reset to default"
            >
                <i class="fa-solid fa-rotate-left"></i>
            </button>
        </div>
    </div>
</div>

<style>
    .row {
        display: grid;
        grid-template-columns: minmax(0, 1fr) auto;
        gap: 16px;
        align-items: center;
        padding: 10px 12px;
        border-bottom: 1px solid color-mix(in srgb, var(--bg-hover) 50%, transparent);
    }
    .row:last-child { border-bottom: none; }

    .label-col { min-width: 0; }
    .label { font-size: 13px; color: var(--text); }
    .desc { font-size: 11px; color: var(--text-muted); margin-top: 2px; }

    .trigger-col {
        display: flex;
        gap: 18px;
        align-items: flex-start;
    }
    .trigger-cell {
        display: inline-flex;
        align-items: flex-start;
        gap: 4px;
    }

    .reset {
        width: 22px;
        height: 22px;
        border: none;
        background: transparent;
        color: var(--text-muted);
        border-radius: 4px;
        cursor: pointer;
        font-size: 10px;
        opacity: 0;
        transition: opacity 0.15s, background 0.15s, color 0.15s;
    }
    .reset.visible { opacity: 1; }
    .reset:hover:not(:disabled) { background: var(--bg-hover); color: var(--text); }
    .reset:disabled { cursor: default; }
</style>
