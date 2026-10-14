<script lang="ts">
    /**
     * The colour panel's Jitter tab: three amplitudes read from and written
     * to the `colors.jitter*` prefs. Each brush stroke draws one offset per
     * channel in `[-amp, amp]` at stroke start (`brush::color_jitter`), so
     * the engine only ever reads the settled value: a drag is held here and
     * the pref is written when the artist lets go.
     */
    import { config } from '../../config/store.svelte';
    import Scrub from '../Scrub.svelte';

    interface Control {
        key: string;
        label: string;
        icon: string;
        max: number;
        format: (v: number) => string;
        title: string;
    }

    const CONTROLS: Control[] = [
        {
            key: 'colors.jitterHue',
            label: 'Hue',
            icon: 'fa6-solid:palette',
            max: 180,
            format: (v) => `${Math.round(v)}°`,
            title: 'Degrees either side of the foreground hue a stroke may land',
        },
        {
            key: 'colors.jitterSaturation',
            label: 'Saturation',
            icon: 'fa6-solid:droplet',
            max: 1,
            format: (v) => `${Math.round(v * 100)}%`,
            title: "How far a stroke's saturation may drift from the foreground's",
        },
        {
            key: 'colors.jitterValue',
            label: 'Value',
            icon: 'fa6-solid:sun',
            max: 1,
            format: (v) => `${Math.round(v * 100)}%`,
            title: "How far a stroke's value may drift from the foreground's",
        },
    ];

    /** Values mid-drag, by pref key; absent once the drag has settled. */
    let dragging = $state<Record<string, number>>({});

    function valueOf(c: Control): number {
        return dragging[c.key] ?? config.number(c.key, 0);
    }

    function commit(c: Control, v: number) {
        delete dragging[c.key];
        config.set(c.key, v);
    }
</script>

<div class="jitter">
    {#each CONTROLS as c (c.key)}
        <Scrub
            mode="drag"
            icon={c.icon}
            label={c.label}
            value={valueOf(c)}
            min={0}
            max={c.max}
            default={0}
            formatValue={c.format}
            onChange={(v) => (dragging[c.key] = v)}
            onCommit={(v) => commit(c, v)}
            title={c.title}
        />
    {/each}
</div>

<style>
    .jitter {
        display: flex;
        flex-direction: column;
        gap: 8px;
        width: 100%;
    }
</style>
