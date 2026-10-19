<!--
  The actions and hotkeys table.

  The one interactive page in the reference section, because "searchable" was in
  the requirement. Every other generated page is a static markdown table that
  Pagefind already indexes; this one has over a hundred rows, so filtering in
  the page beats scrolling.

  Its keys follow the reader's hotkey choice like every other key in the docs:
  each cell carries every preset's key and CSS shows the chosen one
  (lib/hotkeys.ts), so the table is right before it hydrates and with no
  JavaScript at all. The choice is made in the header; the table only reads it,
  so the filter matches the key the reader sees.

  Its data is built at generation time by scripts/gen-docs.mjs: rows already
  joined to their chords, icons already resolved to inline SVG. Nothing here
  knows about Darkly's registries, catalogs, or hotkey resolution; that all
  happened on the Rust side, in `export-docs`.
-->
<script lang="ts">
    import { onMount } from 'svelte';
    import data from '../../generated/docs-actions.json';
    import {
        DEFAULT_OS,
        chordText,
        current,
        defaultPreset,
        hotkeyHtml,
        onChoice,
        presetLabelHtml,
        type Chord,
    } from '../../lib/hotkeys';

    interface Row {
        id: string;
        name: string;
        description: string;
        category: string;
        iconSvg: string;
        source: string;
        chords: Record<string, Chord[]>;
    }

    const presets: string[] = data.presets;
    const rows = data.rows as Row[];

    let query = $state('');
    // The filter's view of the choice in the header, read once mounted; the
    // server renders the default.
    let choice = $state({ preset: defaultPreset(presets), os: DEFAULT_OS });

    const needle = $derived(query.trim().toLowerCase());
    const shown = $derived(
        !needle
            ? rows
            : rows.filter((row) =>
                  `${row.name} ${row.description} ${row.category} ${row.id} ${chordText(row.chords[choice.preset], choice.os)}`
                      .toLowerCase()
                      .includes(needle),
              ),
    );

    onMount(() => {
        const read = () => (choice = current(presets));
        read();
        return onChoice(read);
    });
</script>

<!--
  `not-content` opts this component out of the page's markdown styling.

  The name is Starlight's escape hatch, for the same problem: a content layer
  that styles *descendants*, such as
  `.sl-markdown-content :not(a, strong, …) + :not(a, strong, …)`, puts
  `margin-top: 1rem` on every adjacent sibling pair anywhere on the page, which
  inside a component pushes each control after the first a whole line below
  its neighbour.

  styles/prose.css's spacing is `.prose > * + *`, direct children only, which
  stops at this wrapper. So the class guards only the table rules, which select
  `table`/`th`/`td` deeply enough to reach the one below. Keep it: it is one
  word standing between this component and the next stylesheet that gets
  careless.
-->
<div class="not-content">
<div class="controls">
    <input
        type="search"
        bind:value={query}
        placeholder="Filter {rows.length} actions…"
        aria-label="Filter actions"
    />
</div>

<div class="scroller">
    <table>
        <thead>
            <tr>
                <th class="icon"><span class="sr-only">Icon</span></th>
                <th>Action</th>
                <th>Description</th>
                <th class="chord">Hotkey</th>
            </tr>
        </thead>
        <tbody>
            {#each shown as row (row.id)}
                <tr>
                    <td class="icon">{@html row.iconSvg}</td>
                    <td class="name">
                        <strong>{row.name}</strong>
                        <span class="category">{row.category}</span>
                    </td>
                    <td>{row.description}</td>
                    <td class="chord">{@html hotkeyHtml(row.chords, presets)}</td>
                </tr>
            {/each}
        </tbody>
    </table>
</div>

{#if shown.length === 0}
    <p class="empty">No action matches “{query}”.</p>
{:else}
    <p class="count">
        {shown.length} of {rows.length} actions ·
        {@html presetLabelHtml(presets)} preset (change it at the top of the page) · Darkly {data.version}
    </p>
{/if}
</div>

<style>
    .controls {
        display: flex;
        flex-wrap: wrap;
        gap: 0.75rem;
        align-items: center;
        margin-block: 1rem;
    }

    input[type='search'] {
        flex: 1 1 16rem;
        min-width: 0;
        padding: 0.5rem 0.75rem;
        border: 1px solid var(--bg-active);
        border-radius: 0.375rem;
        background: var(--bg-raised);
        color: var(--text);
        font: inherit;
        font-size: 0.875rem;
        line-height: 1.4;
    }

    /* A long description column beside three narrow ones still overruns a phone.
       The table scrolls inside its own box rather than the page scrolling. */
    .scroller {
        overflow-x: auto;
        border: 1px solid var(--bg-active);
        border-radius: 0.5rem;
    }

    table {
        width: 100%;
        margin: 0;
        border-collapse: collapse;
        font-size: 0.875rem;
    }

    th,
    td {
        padding: 0.5rem 0.75rem;
        text-align: left;
        border-bottom: 1px solid var(--bg-hover);
        vertical-align: top;
    }

    thead th {
        position: sticky;
        top: 0;
        background: var(--bg);
        white-space: nowrap;
    }

    tbody tr:last-child td {
        border-bottom: 0;
    }

    td.icon,
    th.icon {
        width: 2.25rem;
        padding-right: 0;
        color: var(--text);
    }

    .name strong {
        display: block;
        white-space: nowrap;
    }

    .category {
        color: var(--text-muted);
        font-size: 0.75rem;
    }

    .chord {
        white-space: nowrap;
    }

    .count,
    .empty {
        margin-top: 0.75rem;
        color: var(--text-muted);
        font-size: 0.875rem;
    }

    .sr-only {
        position: absolute;
        width: 1px;
        height: 1px;
        overflow: hidden;
        clip-path: inset(50%);
        white-space: nowrap;
    }
</style>
