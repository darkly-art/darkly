<script lang="ts">
    import { config, formatHotkey } from '../../../config/store.svelte';
    import { actions, sites, contextSatisfied, type ActionRegistration } from '../../../actions/registry';
    import { parseBinding } from '../../../config/hotkeys.svelte';

    type Props = {
        /** The pref key being edited (e.g. "hotkeys.brushTool"). Required for
         *  conflict detection so we can compare against every other binding. */
        prefKey: string;
        value: string;
        onchange: (v: string) => void;
        /** Action this row belongs to — needed to filter compatible binding
         *  sites for the scope dropdown. */
        action?: ActionRegistration;
    };
    let { prefKey, value, onchange, action }: Props = $props();

    const parts = $derived.by(() => parseBinding(value));

    /** Sites whose `provides` is a superset of `action.requires` — only those
     *  can supply the context this action needs. `keyboard` is excluded
     *  because it's the implicit global fallback (selected via "(global)"). */
    const compatibleSites = $derived.by(() => {
        if (!action) return [];
        return sites.all().filter(s =>
            s.name !== 'keyboard'
            && contextSatisfied(action, s.provides),
        );
    });

    let capturing = $state(false);

    /** True if some other action's binding currently equals our value. */
    const conflict = $derived.by(() => {
        if (!value) return null;
        // Bust on every config mutation.
        void config.get('');
        const ownActionId = prefKey.startsWith('hotkeys.') ? prefKey.slice('hotkeys.'.length) : null;
        const colliders: string[] = [];
        for (const other of actions.all()) {
            if (other.id === ownActionId) continue;
            const otherVal = config.get(`hotkeys.${other.id}`);
            if (otherVal === value) colliders.push(other.displayName);
        }
        if (colliders.length === 0) return null;
        return `Also bound to: ${colliders.join(', ')}`;
    });

    function pickSite(e: Event) {
        const site = (e.currentTarget as HTMLSelectElement).value;
        if (site === '') {
            // "(global)" — bare chord, no scope prefix.
            onchange(parts.chord);
        } else {
            // Keep the chord if we have one; otherwise emit `<site>:`
            // (an empty-chord form, treated as unbound at dispatch).
            onchange(parts.chord ? `${site}:${parts.chord}` : `${site}:`);
        }
    }

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

        const partsArr: string[] = [];
        if (e.ctrlKey || e.metaKey) partsArr.push('$mod');
        if (e.shiftKey) partsArr.push('Shift');
        if (e.altKey) partsArr.push('Alt');
        partsArr.push(e.code);
        const chord = partsArr.join('+');
        // Preserve the currently-selected scope.
        onchange(parts.site ? `${parts.site}:${chord}` : chord);
        capturing = false;
    }

    const displayedChord = $derived(formatHotkey(parts.chord) ?? '(unbound)');
</script>

<div class="hotkey-row">
    {#if compatibleSites.length > 0}
        <select class="site" value={parts.site ?? ''} onchange={pickSite} title="Scope this hotkey to a UI region (or leave global)">
            <option value="">(global)</option>
            {#each compatibleSites as s (s.name)}
                <option value={s.name}>{s.displayName ?? s.name}</option>
            {/each}
        </select>
    {/if}
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
            <span class="value">{displayedChord}</span>
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
    .site {
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        color: var(--text);
        border-radius: 4px;
        padding: 5px 8px;
        font-size: 12px;
        min-width: 100px;
    }
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
