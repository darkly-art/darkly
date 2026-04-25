<script lang="ts">
    import { config, formatHotkey } from '../../../config/store.svelte';
    import { actions } from '../../../actions/registry';

    type Props = {
        /** The pref key being edited (e.g. "hotkeys.brushTool"). Required for
         *  conflict detection so we can compare against every other binding. */
        prefKey: string;
        value: string;
        onchange: (v: string) => void;
    };
    let { prefKey, value, onchange }: Props = $props();

    let capturing = $state(false);

    /** True if some other action's binding currently equals our value. */
    const conflict = $derived.by(() => {
        if (!value) return null;
        // Bust on every config mutation.
        void config.get('');
        const ownActionId = prefKey.startsWith('hotkeys.') ? prefKey.slice('hotkeys.'.length) : null;
        const colliders: string[] = [];
        for (const action of actions.all()) {
            if (action.id === ownActionId) continue;
            const other = config.get(`hotkeys.${action.id}`);
            if (other === value) colliders.push(action.displayName);
        }
        if (colliders.length === 0) return null;
        return `Also bound to: ${colliders.join(', ')}`;
    });

    function beginCapture() {
        capturing = true;
    }

    function stopCapture() {
        capturing = false;
    }

    function onKeyDown(e: KeyboardEvent) {
        if (!capturing) return;
        e.preventDefault();
        e.stopPropagation();

        // Escape = cancel capture, keep old value.
        if (e.code === 'Escape') { capturing = false; return; }

        // Backspace / Delete = clear binding.
        if (e.code === 'Backspace' || e.code === 'Delete') {
            onchange('');
            capturing = false;
            return;
        }

        // Ignore pure-modifier presses — they're prefixes, not keystrokes.
        if (['ShiftLeft','ShiftRight','ControlLeft','ControlRight','AltLeft','AltRight','MetaLeft','MetaRight'].includes(e.code)) {
            return;
        }

        const parts: string[] = [];
        if (e.ctrlKey || e.metaKey) parts.push('$mod');
        if (e.shiftKey) parts.push('Shift');
        if (e.altKey) parts.push('Alt');
        parts.push(e.code);
        onchange(parts.join('+'));
        capturing = false;
    }

    const displayed = $derived(formatHotkey(value) ?? '(unbound)');
</script>

<div class="hotkey-row">
    <button
        type="button"
        class="capture"
        class:capturing
        class:has-conflict={!!conflict}
        onclick={beginCapture}
        onblur={stopCapture}
        onkeydown={onKeyDown}
        title={conflict ?? 'Click, then press a key combination'}
    >
        {#if capturing}
            <span class="hint">Press a key…</span>
        {:else}
            <span class="value">{displayed}</span>
        {/if}
    </button>
    {#if conflict && !capturing}
        <span class="conflict-note" title={conflict}>
            <i class="fa-solid fa-triangle-exclamation"></i>
        </span>
    {/if}
</div>

<style>
    .hotkey-row { display: inline-flex; align-items: center; gap: 6px; }
    .capture {
        font-family: var(--font-mono, monospace);
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        color: var(--text);
        cursor: pointer;
        padding: 5px 10px;
        font-size: 12px;
        min-width: 140px;
        text-align: left;
    }
    .capture:hover { border-color: color-mix(in srgb, var(--accent) 60%, var(--bg-hover)); }
    .capture.capturing {
        border-color: var(--accent);
        background: color-mix(in srgb, var(--accent) 10%, var(--bg-hover));
    }
    .capture.has-conflict { border-color: var(--danger, #e74c3c); }
    .hint { color: var(--text-muted); font-style: italic; }
    .value { color: var(--text); }
    .conflict-note { color: var(--danger, #e74c3c); font-size: 12px; }
</style>
