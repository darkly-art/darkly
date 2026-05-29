<script lang="ts">
    import type { PrefInfo } from '../../config/schema';
    import { config } from '../../config/store.svelte';
    import BoolToggle from './widgets/BoolToggle.svelte';
    import NumberSlider from './widgets/NumberSlider.svelte';
    import NumberInput from './widgets/NumberInput.svelte';
    import TextInput from './widgets/TextInput.svelte';
    import EnumDropdown from './widgets/EnumDropdown.svelte';
    import ColorInput from './widgets/ColorInput.svelte';
    import HotkeyCapture from './widgets/HotkeyCapture.svelte';

    type Props = { pref: PrefInfo };
    let { pref }: Props = $props();

    const value = $derived(config.get(pref.key));
    const hasOverride = $derived(config.hasOverride(pref.key));
    /** Layer-below-user value (overlay → defaults). Drives the tooltip's
     *  "Reset to …" description so the user can see what would be revealed. */
    const baseValue = $derived(config.baseValue(pref.key));
    const resetTitle = $derived(
        baseValue == null ? 'Reset' : `Reset to ${String(baseValue)}`,
    );

    function onchange(v: unknown) {
        config.set(pref.key, v);
    }

    function reset() {
        config.resetKey(pref.key);
    }
</script>

{#if pref.widget !== 'hidden'}
    <div class="pref-row">
        <div class="label-col">
            <div class="label">{pref.displayName}</div>
            {#if pref.description}<div class="desc">{pref.description}</div>{/if}
        </div>
        <div class="widget-col">
            {#if pref.widget === 'color'}
                <ColorInput value={value as string} {onchange} />
            {:else if pref.widget === 'hotkey'}
                <HotkeyCapture prefKey={pref.key} value={value as string} {onchange} />
            {:else if pref.kind === 'bool'}
                <BoolToggle value={value as boolean} {onchange} />
            {:else if pref.kind === 'enum'}
                <EnumDropdown
                    value={value as string}
                    options={pref.options ?? []}
                    {onchange}
                />
            {:else if pref.kind === 'int' || pref.kind === 'float'}
                {#if pref.widget === 'numberInput'}
                    <NumberInput
                        value={value as number}
                        min={pref.min}
                        max={pref.max}
                        integer={pref.kind === 'int'}
                        {onchange}
                    />
                {:else}
                    <NumberSlider
                        value={value as number}
                        min={pref.min ?? 0}
                        max={pref.max ?? 1}
                        integer={pref.kind === 'int'}
                        {onchange}
                    />
                {/if}
            {:else}
                <TextInput value={value as string} {onchange} />
            {/if}
        </div>
        <button
            type="button"
            class="reset"
            class:visible={hasOverride}
            title={resetTitle}
            onclick={reset}
            disabled={!hasOverride}
        >
            <i class="fa-solid fa-rotate-left"></i>
        </button>
    </div>
{/if}

<style>
    .pref-row {
        display: grid;
        grid-template-columns: minmax(0, 1fr) minmax(200px, auto) 28px;
        gap: 16px;
        align-items: center;
        padding: 10px 12px;
        border-bottom: 1px solid color-mix(in srgb, var(--bg-hover) 50%, transparent);
    }
    .pref-row:last-child { border-bottom: none; }

    .label-col { min-width: 0; }
    .label { font-size: 13px; color: var(--text); }
    .desc { font-size: 11px; color: var(--text-muted); margin-top: 2px; }
    .widget-col { display: flex; justify-content: flex-start; }
    .reset {
        width: 24px;
        height: 24px;
        border: none;
        background: transparent;
        color: var(--text-muted);
        border-radius: 4px;
        cursor: pointer;
        font-size: 11px;
        opacity: 0;
        transition: opacity 0.15s, background 0.15s, color 0.15s;
    }
    .reset.visible { opacity: 1; }
    .reset:hover { background: var(--bg-hover); color: var(--text); }
    .reset:disabled { cursor: default; }
</style>
