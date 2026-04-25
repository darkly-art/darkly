<script lang="ts">
    import { actions, sites } from '../../../actions/registry';

    type Props = {
        /** The pref key, e.g. "bindings.layerEye.alt+click". */
        prefKey: string;
        value: string;
        onchange: (v: string) => void;
    };
    let { prefKey, value, onchange }: Props = $props();

    // Extract the site name (middle part): bindings.<site>.<chord>
    const siteName = $derived(prefKey.split('.')[1] ?? '');
    const site = $derived(sites.get(siteName));
    const available = $derived(site ? actions.compatibleWith(site) : actions.all());
</script>

<select
    {value}
    onchange={(e) => onchange(e.currentTarget.value)}
>
    <option value="">(none)</option>
    {#each available as action (action.id)}
        <option value={action.id}>{action.displayName}</option>
    {/each}
</select>

<style>
    select {
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        color: var(--text);
        border-radius: 4px;
        padding: 5px 8px;
        font-size: 12px;
        min-width: 180px;
    }
    select:focus { outline: 2px solid var(--accent); outline-offset: 0; border-color: transparent; }
</style>
