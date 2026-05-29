<script lang="ts">
    import { config } from '../../config/store.svelte';
    import {
        sites,
        contextSatisfied,
        type ActionRegistration,
    } from '../../actions/registry';
    import {
        readTriggers,
        writeTriggers,
        resetTriggers,
        hasTriggerOverride,
        findTriggerConflicts,
        detectKind,
        siteOf,
        chordOf,
        type Trigger,
        type TriggerKind,
    } from '../../actions/triggers_combined';
    import ChordCapture from './widgets/ChordCapture.svelte';

    type Props = {
        action: ActionRegistration;
        /** When true, each row shows an editable scope <select>. When
         *  false, non-global scope is still surfaced via a small read-only
         *  chip so the user knows the binding isn't unconditional. */
        showScope: boolean;
    };
    let { action, showScope }: Props = $props();

    /** Compatible binding sites for this action — i.e. sites whose
     *  `provides` is a superset of `action.requires`. `keyboard` is
     *  excluded because it's the implicit global fallback (selected via
     *  "Anywhere"). The "Anywhere" option itself is offered iff the
     *  `keyboard` site satisfies the action's requirements. */
    const compatibleSites = $derived.by(() =>
        sites.all().filter(s =>
            s.name !== 'keyboard'
            && contextSatisfied(action, s.provides),
        ),
    );

    const allowsAnywhere = $derived.by(() => {
        const kbd = sites.get('keyboard');
        return kbd ? contextSatisfied(action, kbd.provides) : false;
    });

    /** Persistent rows — read from config and re-evaluated on every
     *  store mutation via the `void config.get('')` reactivity tap. */
    const stored = $derived.by(() => {
        void config.get('');
        return readTriggers(action.id);
    });

    /** Ephemeral rows — added by "+ Add trigger" but not yet captured into.
     *  Lives in component state instead of config because
     *  `serializeTriggers` filters out empty-chord rows on write (to keep
     *  storage clean of ghost overrides). If we wrote a fresh empty row
     *  through config, it'd be filtered out before the reactive re-read
     *  could surface it, and "Add trigger" would visibly do nothing.
     *  Promoted to `stored` the moment the user captures a chord; dropped
     *  silently on ×, modal close, or reset. */
    let pending = $state<Trigger[]>([]);

    /** Visible list = persistent rows followed by any pending ones. */
    const triggers = $derived([...stored, ...pending]);

    const overridden = $derived.by(() => {
        void config.get('');
        return hasTriggerOverride(action.id);
    });

    /** One conflict label per trigger (or null). */
    const conflicts = $derived.by<(string | null)[]>(() => {
        void config.get('');
        return triggers.map(t => {
            const colliders = findTriggerConflicts(t.binding, action.id);
            return colliders.length > 0
                ? `Also bound to: ${colliders.join(', ')}`
                : null;
        });
    });

    /** Index of the row to auto-start capture on. Set by "+ Add trigger"
     *  to save the user a click; cleared on next mutation. */
    let pendingAutostart = $state<number | null>(null);

    /** Compose a binding string from kind + site + chord. Mouse chords
     *  always need a site; keyboard chords keep the site they had (or
     *  remain global). */
    function composeBinding(kind: TriggerKind, site: string | null, chord: string): string {
        if (!chord) return '';
        if (kind === 'mouse' && !site) {
            // A mouse chord without a site can't dispatch (dispatchClick
            // demands a site). Auto-pick the first compatible site.
            site = compatibleSites[0]?.name ?? null;
        }
        return site ? `${site}:${chord}` : chord;
    }

    /** Index `i` in the combined visible list points to either `stored`
     *  (i < stored.length) or `pending` (i - stored.length otherwise). */
    function isPendingIndex(i: number): boolean {
        return i >= stored.length;
    }

    function updateTrigger(index: number, next: Trigger) {
        pendingAutostart = null;
        if (isPendingIndex(index)) {
            // First write to a freshly-added row: promote it into stored.
            // (If `next.binding` is empty we still promote so the caller
            // doesn't have to special-case clear-on-pending; serialization
            // filters it back out before it reaches storage.)
            const pIdx = index - stored.length;
            const promoted = [...stored];
            promoted.push(next);
            pending = pending.toSpliced(pIdx, 1);
            writeTriggers(action.id, promoted);
        } else {
            const list = stored.slice();
            list[index] = next;
            writeTriggers(action.id, list);
        }
    }

    function removeTrigger(index: number) {
        pendingAutostart = null;
        if (isPendingIndex(index)) {
            const pIdx = index - stored.length;
            pending = pending.toSpliced(pIdx, 1);
        } else {
            const list = stored.slice();
            list.splice(index, 1);
            writeTriggers(action.id, list);
        }
    }

    function addTrigger() {
        // Default new trigger: keyboard, global if allowed, else first
        // compatible site (chord empty until captured).
        const initialSite = allowsAnywhere ? null : (compatibleSites[0]?.name ?? null);
        const binding = initialSite ? `${initialSite}:` : '';
        pending = [...pending, { kind: 'kbd', binding } satisfies Trigger];
        // Autostart index = position in the combined list.
        pendingAutostart = stored.length + pending.length - 1;
    }

    function onCapture(index: number, newChord: string) {
        if (!newChord) {
            // Cleared via Backspace/Delete — drop the row entirely so the
            // user doesn't end up with a ghost row that can't dispatch.
            removeTrigger(index);
            return;
        }
        const old = triggers[index];
        const newKind = detectKind(newChord);
        const newSite = siteOf(old.binding);
        const newBinding = composeBinding(newKind, newSite, newChord);
        updateTrigger(index, { kind: newKind, binding: newBinding });
    }

    function onPickSite(index: number, newSite: string) {
        const old = triggers[index];
        const chord = chordOf(old.binding);
        const site = newSite === '' ? null : newSite;
        // Changing site doesn't change kind. composeBinding handles the
        // mouse-without-site safety net.
        updateTrigger(index, {
            kind: old.kind,
            binding: composeBinding(old.kind, site, chord),
        });
    }

    function reset() {
        resetTriggers(action.id);
        pending = [];
        pendingAutostart = null;
    }

    function siteLabel(siteName: string): string {
        return sites.get(siteName)?.displayName ?? siteName;
    }
</script>

<div class="trigger-list">
    {#each triggers as t, i (`${t.kind}:${t.binding}:${i}`)}
        {@const site = siteOf(t.binding)}
        {@const chord = chordOf(t.binding)}
        {@const allowAnywhereHere = allowsAnywhere && t.kind === 'kbd'}
        <div class="row">
            {#if showScope}
                <select
                    class="scope"
                    value={site ?? ''}
                    onchange={(e) => onPickSite(i, (e.currentTarget as HTMLSelectElement).value)}
                    title="Where this trigger fires"
                >
                    {#if allowAnywhereHere}
                        <option value="">Anywhere</option>
                    {/if}
                    {#each compatibleSites as s (s.name)}
                        <option value={s.name}>On {s.displayName ?? s.name}</option>
                    {/each}
                </select>
            {:else if site}
                <span class="scope-chip" title="Scope (toggle 'Show scopes' to edit)">
                    On {siteLabel(site)}
                </span>
            {/if}

            <ChordCapture
                value={chord}
                onchange={(c) => onCapture(i, c)}
                autostart={i === pendingAutostart}
                conflict={conflicts[i]}
            />

            {#if conflicts[i]}
                <span class="conflict-note" title={conflicts[i] ?? ''}>
                    <i class="fa-solid fa-triangle-exclamation"></i>
                </span>
            {/if}

            <button
                type="button"
                class="remove"
                onclick={() => removeTrigger(i)}
                title="Remove this trigger"
            >
                <i class="fa-solid fa-xmark"></i>
            </button>
        </div>
    {/each}

    <div class="footer">
        <button type="button" class="add" onclick={addTrigger}>
            <i class="fa-solid fa-plus"></i> Add trigger
        </button>
        <button
            type="button"
            class="reset"
            class:visible={overridden}
            disabled={!overridden}
            onclick={reset}
            title="Reset to default triggers"
        >
            <i class="fa-solid fa-rotate-left"></i>
        </button>
    </div>
</div>

<style>
    .trigger-list {
        display: flex;
        flex-direction: column;
        gap: 2px;
        align-items: flex-end;
    }

    .row {
        display: flex;
        align-items: center;
        gap: 6px;
    }

    .scope, .scope-chip {
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        color: var(--text);
        border-radius: 4px;
        padding: 5px 8px;
        font-size: 12px;
    }
    .scope { min-width: 130px; }
    .scope-chip {
        color: var(--text-muted);
        font-size: 11px;
        white-space: nowrap;
    }

    .conflict-note { color: var(--danger, #e74c3c); font-size: 12px; }

    .remove {
        width: 22px;
        height: 22px;
        border: none;
        background: transparent;
        color: var(--text-muted);
        border-radius: 4px;
        cursor: pointer;
        font-size: 10px;
    }
    .remove:hover { background: var(--bg-hover); color: var(--text); }

    .footer {
        display: flex;
        align-items: center;
        gap: 6px;
    }
    .add {
        background: transparent;
        border: 1px dashed color-mix(in srgb, var(--text-muted) 50%, transparent);
        color: var(--text-muted);
        border-radius: 4px;
        padding: 1px 10px;
        font-size: 11px;
        line-height: 1.4;
        cursor: pointer;
    }
    .add:hover {
        border-color: var(--accent);
        color: var(--text);
    }
    .reset {
        width: 20px;
        height: 18px;
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
