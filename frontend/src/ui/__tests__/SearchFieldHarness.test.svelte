<script lang="ts">
    /**
     * Test host for `SearchField`: holds the query the way every real caller
     * does, and reports it out so the binding can be observed.
     */
    import SearchField from '../SearchField.svelte';

    let { onquery }: { onquery: (q: string) => void } = $props();

    let query = $state('');
    let el = $state<HTMLInputElement | null>(null);

    $effect(() => {
        onquery(query);
    });

    /** The input itself, to prove `element` reaches the caller. */
    export function input(): HTMLInputElement | null {
        return el;
    }
</script>

<SearchField bind:value={query} bind:element={el} placeholder="Search things…" />
