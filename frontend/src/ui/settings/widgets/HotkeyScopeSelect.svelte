<script lang="ts">
    import { sites, contextSatisfied, type ActionRegistration } from '../../../actions/registry';
    import { parseBinding } from '../../../config/hotkeys.svelte';

    type Props = {
        /** The full binding (`<site>:<chord>` or bare chord). */
        value: string;
        onchange: (v: string) => void;
        action: ActionRegistration;
    };
    let { value, onchange, action }: Props = $props();

    const parts = $derived.by(() => parseBinding(value ?? ''));

    /** Sites whose `provides` is a superset of `action.requires`. `keyboard`
     *  is excluded — it's the implicit global fallback (selected via
     *  "(global)"). */
    const compatibleSites = $derived.by(() =>
        sites.all().filter(s =>
            s.name !== 'keyboard'
            && contextSatisfied(action, s.provides),
        ),
    );

    function pickSite(e: Event) {
        const site = (e.currentTarget as HTMLSelectElement).value;
        if (site === '') {
            onchange(parts.chord);
        } else {
            onchange(parts.chord ? `${site}:${parts.chord}` : `${site}:`);
        }
    }
</script>

{#if compatibleSites.length > 0}
    <select
        class="site"
        value={parts.site ?? ''}
        onchange={pickSite}
        title="Scope this hotkey to a UI region (or leave global)"
    >
        <option value="">(global)</option>
        {#each compatibleSites as s (s.name)}
            <option value={s.name}>{s.displayName ?? s.name}</option>
        {/each}
    </select>
{/if}

<style>
    .site {
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        color: var(--text);
        border-radius: 4px;
        padding: 5px 8px;
        font-size: 12px;
        min-width: 100px;
    }
</style>
