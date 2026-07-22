<script lang="ts">
    import MaskChainControl from '../MaskChainControl.svelte';
    import type { MaskLinkState } from '../maskChain';

    let { initialLinked = true, editable = true, active = false, refreshImmediately = false, ontoggle, onselect }: {
        initialLinked?: boolean;
        editable?: boolean;
        active?: boolean;
        refreshImmediately?: boolean;
        ontoggle?: (linked: boolean) => void;
        onselect?: () => void;
    } = $props();

    // svelte-ignore state_referenced_locally
    let linkedToHost = $state(initialLinked);
    let mask = $derived<MaskLinkState>({ id: 42, linkedToHost, editable });

    export function project(linked: boolean) {
        linkedToHost = linked;
    }
</script>

<div
    data-testid="row"
    role="button"
    tabindex="-1"
    onclick={() => onselect?.()}
    onkeydown={() => {}}
>
    <MaskChainControl
        {mask}
        thumbnail="data:image/png;base64,AA=="
        {active}
        enabled={true}
        onselect={() => onselect?.()}
        oncontextmenu={() => {}}
        onupdate={() => {
            const linked = !linkedToHost;
            ontoggle?.(linked);
            if (refreshImmediately) linkedToHost = linked;
        }}
    />
</div>
