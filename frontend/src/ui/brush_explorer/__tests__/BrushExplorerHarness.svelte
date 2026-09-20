<script lang="ts">
    // Drives the explorer's `open` prop from outside the component tree, which
    // is what the open/close/reopen sequence under test needs. A harness rather
    // than a reactive props object because runes are not available in a `.ts`
    // test file.
    import BrushExplorer from '../BrushExplorer.svelte';

    let open = $state(false);

    export function setOpen(next: boolean) {
        open = next;
    }
</script>

<BrushExplorer bind:open />
