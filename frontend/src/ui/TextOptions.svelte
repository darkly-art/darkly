<script lang="ts">
    import { onMount } from 'svelte';
    import { app } from '../state/app.svelte';
    import { textSession, pushStyleEdit } from '../tools/text.svelte';
    import EnumDropdown from './settings/widgets/EnumDropdown.svelte';
    import Scrub from './Scrub.svelte';
    import ToolBarLayout from './ToolBarLayout.svelte';

    const ALIGN_OPTIONS: [string, string][] = [
        ['start', 'Left'],
        ['center', 'Center'],
        ['end', 'Right'],
        ['justified', 'Justify'],
    ];

    let fontOptions = $state<[string, string][]>([['Noto Sans', 'Noto Sans']]);

    onMount(async () => {
        if (!app.engine) return;
        const res = (await app.engine.send('list_fonts')) as { fonts: string[] } | null;
        if (res?.fonts?.length) {
            fontOptions = res.fonts.map((f) => [f, f] as [string, string]);
            if (!res.fonts.includes(textSession.fontFamily)) {
                textSession.fontFamily = res.fonts[0];
            }
        }
    });
</script>

<ToolBarLayout>
    {#snippet center()}
        <label class="row" title="Font family">
            <span>Font</span>
            <EnumDropdown
                value={textSession.fontFamily}
                options={fontOptions}
                onchange={(v) => {
                    textSession.fontFamily = v;
                    void pushStyleEdit({ font_family: v });
                }}
            />
        </label>
        <Scrub
            mode="drag"
            label="Size"
            value={textSession.size}
            min={4}
            max={512}
            default={48}
            formatValue={(v) => String(Math.round(v))}
            onChange={(v) => {
                const s = Math.round(v);
                textSession.size = s;
                void pushStyleEdit({ size: s });
            }}
            title="Font size in canvas pixels."
        />
        <label class="row" title="Horizontal alignment">
            <span>Align</span>
            <EnumDropdown
                value={textSession.align}
                options={ALIGN_OPTIONS}
                onchange={(v) => {
                    textSession.align = v;
                    void pushStyleEdit({ align: v });
                }}
            />
        </label>
        <label class="row" title="Italic">
            <input
                type="checkbox"
                checked={textSession.italic}
                onchange={(e) => {
                    const it = (e.currentTarget as HTMLInputElement).checked;
                    textSession.italic = it;
                    void pushStyleEdit({ italic: it });
                }}
            />
            <span>Italic</span>
        </label>
    {/snippet}
</ToolBarLayout>

<style>
    .row {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        font-size: 12px;
        color: var(--text);
    }
</style>
