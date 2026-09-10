<script lang="ts">
    /**
     * Test host for `ColorWheel`: feeds the wheel's own output straight back
     * into its `value`, the round trip through RGB every real host performs.
     */
    import { untrack } from 'svelte';
    import type { Color } from '../../state/app.svelte';
    import ColorWheel from '../color/ColorWheel.svelte';

    let { initial, onvalue, size }: { initial: Color; onvalue: (c: Color) => void; size: number } = $props();

    let value = $state<Color>(untrack(() => initial));

    function set(c: Color) {
        value = c;
        onvalue(c);
    }

    /** An external write, as from the eyedropper or a reset. */
    export function setValue(c: Color) {
        value = c;
    }
</script>

<ColorWheel {value} oninput={set} onchange={set} {size} />
