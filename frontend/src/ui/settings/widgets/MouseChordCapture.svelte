<script lang="ts">
    import { sites, contextSatisfied, type ActionRegistration } from '../../../actions/registry';

    type Props = {
        action: ActionRegistration;
        /** Current value: `"<site>:<chord>"` or empty. */
        value: string;
        onchange: (v: string) => void;
    };
    let { action, value, onchange }: Props = $props();

    /** Sites whose `provides` is a superset of `action.accepts` — the only
     *  sites a click can dispatch this action without missing context. */
    const compatibleSites = $derived.by(() => {
        return sites.all().filter(site => contextSatisfied(action, site.provides));
    });

    const parts = $derived.by(() => {
        if (!value) return { site: '', chord: '' };
        const colon = value.indexOf(':');
        if (colon < 0) return { site: '', chord: value };
        return { site: value.slice(0, colon), chord: value.slice(colon + 1) };
    });

    let capturing = $state(false);

    function pickSite(e: Event) {
        const site = (e.currentTarget as HTMLSelectElement).value;
        if (!site) {
            // Choosing "(none)" clears the binding entirely.
            onchange('');
            return;
        }
        // Keep the chord if we have one; otherwise leave it for the user to
        // capture next.
        if (parts.chord) onchange(`${site}:${parts.chord}`);
        else onchange(`${site}:`);
    }

    function beginCaptureChord() {
        if (!parts.site) return;
        capturing = true;
    }

    function chordFromEvent(e: MouseEvent): string {
        const mods: string[] = [];
        if (e.ctrlKey || e.metaKey) mods.push('ctrl');
        if (e.altKey) mods.push('alt');
        if (e.shiftKey) mods.push('shift');
        let interaction: string;
        if (e.button === 1) interaction = 'middleClick';
        else if (e.detail === 2) interaction = 'doubleClick';
        else interaction = 'click';
        return mods.length > 0 ? `${mods.join('+')}+${interaction}` : interaction;
    }

    function captureClick(e: MouseEvent) {
        if (!capturing) return;
        e.preventDefault();
        e.stopPropagation();
        const chord = chordFromEvent(e);
        // Plain click would shadow normal UI clicks; reject it.
        if (chord === 'click') return;
        onchange(`${parts.site}:${chord}`);
        capturing = false;
    }

    function clearChord() {
        // Clear to "site:" so the site stays selected but no chord is bound.
        // (Effectively unbinds — the dispatch path requires a non-empty chord.)
        onchange('');
        capturing = false;
    }

    function formatChord(chord: string): string {
        if (!chord) return '(no click)';
        return chord
            .split('+')
            .map(p => {
                if (p === 'ctrl') return navigator.userAgent.includes('Mac') ? '⌘' : 'Ctrl';
                if (p === 'alt') return navigator.userAgent.includes('Mac') ? '⌥' : 'Alt';
                if (p === 'shift') return navigator.userAgent.includes('Mac') ? '⇧' : 'Shift';
                if (p === 'click') return 'click';
                if (p === 'doubleClick') return 'double-click';
                if (p === 'middleClick') return 'middle-click';
                return p;
            })
            .join('+');
    }
</script>

<div class="row">
    <select value={parts.site} onchange={pickSite} class="site">
        <option value="">(no mouse trigger)</option>
        {#each compatibleSites as site (site.name)}
            <option value={site.name}>{site.name}</option>
        {/each}
    </select>

    {#if parts.site}
        <button
            type="button"
            class="chord"
            class:capturing
            onclick={beginCaptureChord}
            onmousedown={captureClick}
            title={capturing ? 'Click anywhere with your modifiers' : 'Click to set a modifier+click chord'}
        >
            {#if capturing}
                <span class="hint">Click here with modifiers…</span>
            {:else}
                <span class="value">{formatChord(parts.chord)}</span>
            {/if}
        </button>
        {#if parts.chord && !capturing}
            <button type="button" class="clear" onclick={clearChord} title="Clear">
                <i class="fa-solid fa-xmark"></i>
            </button>
        {/if}
    {/if}
</div>

<style>
    .row { display: inline-flex; align-items: center; gap: 6px; }
    .site, .chord, .clear {
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        color: var(--text);
        border-radius: 4px;
        font-size: 12px;
    }
    .site { padding: 5px 8px; min-width: 110px; }
    .chord {
        font-family: var(--font-mono, monospace);
        padding: 5px 10px;
        cursor: pointer;
        min-width: 130px;
        text-align: left;
    }
    .chord:hover { border-color: color-mix(in srgb, var(--accent) 60%, var(--bg-hover)); }
    .chord.capturing {
        border-color: var(--accent);
        background: color-mix(in srgb, var(--accent) 10%, var(--bg-hover));
    }
    .hint { color: var(--text-muted); font-style: italic; }
    .value { color: var(--text); }
    .clear {
        width: 24px;
        height: 24px;
        padding: 0;
        display: inline-flex;
        align-items: center;
        justify-content: center;
        cursor: pointer;
        color: var(--text-muted);
    }
    .clear:hover { color: var(--text); }
</style>
