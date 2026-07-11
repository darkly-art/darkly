<script lang="ts">
    import Slider from '../settings/widgets/Slider.svelte';
    import EnumDropdown from '../settings/widgets/EnumDropdown.svelte';
    import ColorInput from '../settings/widgets/ColorInput.svelte';
    import OffsetPad from '../settings/widgets/OffsetPad.svelte';
    import { hexToRgb01, rgb01ToHex } from '../../lib/color';
    import type { FilterParam, FilterParamValue, ColorValue, Vec2Value } from './filterParams';

    // A single generic scalar/atom param row (label + control). Shared by the
    // filter params editor and each list entry's fields. Mutates `param.value`
    // in place and reports via `oninput` (mid-drag) / `onchange` (commit) — the
    // same contract the channel editors use.
    type Props = {
        param: FilterParam;
        disabled?: boolean;
        oninput?: () => void;
        onchange?: () => void;
    };
    let { param, disabled = false, oninput, onchange }: Props = $props();

    function commit(v: FilterParamValue) {
        param.value = v;
        onchange?.();
    }
    function live(v: FilterParamValue) {
        param.value = v;
        oninput?.();
    }
</script>

<div class="row">
    <span class="label">{param.name}</span>
    {#if param.kind === 'float' || param.kind === 'int'}
        <Slider
            value={(param.value ?? param.default) as number}
            min={(param.min ?? 0) as number}
            max={(param.max ?? 1) as number}
            integer={param.kind === 'int'}
            {disabled}
            onchange={commit}
            format={(v) => (param.kind === 'int' ? String(v) : v.toFixed(2))}
        />
    {:else if param.kind === 'enum'}
        <EnumDropdown
            value={String((param.value ?? param.default) as number)}
            options={((param.options ?? []) as string[]).map((label, i) => [String(i), label])}
            {disabled}
            onchange={(k) => commit(Number(k))}
        />
    {:else if param.kind === 'bool'}
        <input
            type="checkbox"
            class="checkbox"
            checked={(param.value ?? param.default) as boolean}
            {disabled}
            onchange={(e) => commit(e.currentTarget.checked)}
        />
    {:else if param.kind === 'color'}
        <ColorInput
            value={rgb01ToHex((param.value ?? param.default) as ColorValue)}
            onchange={(hex) => commit(hexToRgb01(hex))}
        />
    {:else if param.kind === 'vec2'}
        <OffsetPad
            value={(param.value ?? param.default) as Vec2Value}
            max={(param.max ?? 64) as number}
            oninput={live}
            onchange={commit}
        />
    {/if}
</div>

<style>
    .row {
        display: flex;
        align-items: center;
        gap: 8px;
        min-height: 22px;
    }
    .label {
        font-size: 11px;
        color: var(--text-muted);
        min-width: 56px;
        text-transform: capitalize;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .checkbox {
        accent-color: var(--accent);
    }
</style>
