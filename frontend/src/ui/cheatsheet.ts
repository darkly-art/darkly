/**
 * Hotkey Cheat Sheet — opens a printable, searchable reference of all
 * effective keyboard shortcuts in a separate browser window. Snapshots the
 * action registry + active preset (and current theme) at open time; re-open
 * to see updates.
 */
import { actions, sites } from '../actions/registry';
import { effectiveHotkey, parseBinding } from '../config/hotkeys.svelte';
import { formatHotkey } from '../config/store.svelte';
import { theme } from '../state/theme.svelte';

const CATEGORY_PALETTE = [
    '#3b82f6', '#16a34a', '#9333ea', '#ea580c',
    '#0d9488', '#db2777', '#ca8a04', '#dc2626',
];

function categoryLabel(id: string): string {
    return id.charAt(0).toUpperCase() + id.slice(1);
}

const HTML_ESCAPES: Record<string, string> = {
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;',
};

function esc(s: string): string {
    return s.replace(/[&<>"']/g, c => HTML_ESCAPES[c]);
}

interface Row {
    name: string;
    description: string;
    chord: string;
    /** Display label of the binding site, or `''` for global bindings. */
    scope: string;
}

function titleCase(s: string): string {
    return s.replace(/([A-Z])/g, ' $1').replace(/^./, c => c.toUpperCase()).trim();
}

function siteLabel(name: string): string {
    return sites.get(name)?.displayName ?? titleCase(name);
}

function buildSections(): string {
    let html = '';
    let catIndex = 0;
    for (const [cat, list] of actions.byCategory()) {
        const color = CATEGORY_PALETTE[catIndex % CATEGORY_PALETTE.length];
        catIndex++;

        const rows: Row[] = [];
        for (const a of list) {
            const raw = effectiveHotkey(a.id);
            if (!raw) continue;
            const { site, chord } = parseBinding(raw);
            rows.push({
                name: a.displayName,
                description: a.description ?? '',
                chord: formatHotkey(chord) ?? chord,
                scope: site ? siteLabel(site) : '',
            });
        }
        if (rows.length === 0) continue;
        rows.sort((a, b) => a.name.localeCompare(b.name));

        html += `<section style="--cat:${color}"><h2>${esc(categoryLabel(cat))}</h2><table><tbody>`;
        for (const r of rows) {
            const search = `${r.name} ${r.description} ${r.chord} ${r.scope}`.toLowerCase();
            const desc = r.description ? `<div class="desc">${esc(r.description)}</div>` : '';
            const scopeChip = r.scope
                ? ` <span class="scope">${esc(r.scope)}</span>`
                : '';
            html += `<tr data-search="${esc(search)}"><td class="action"><div class="name">${esc(r.name)}</div>${desc}</td><td class="shortcut"><kbd>${esc(r.chord)}</kbd>${scopeChip}</td></tr>`;
        }
        html += `</tbody></table></section>`;
    }
    return html;
}

const STYLE = `
:root { color-scheme: light dark; }
body {
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    margin: 0;
    padding: 24px;
    background: var(--bg);
    color: var(--text);
}
body.dark {
    --bg: #000000;
    --bg-raised: #111111;
    --bg-hover: #1a1a1a;
    --text: #cccccc;
    --text-muted: #666666;
    --accent: #4d53ff;
}
body.light {
    --bg: #ffffff;
    --bg-raised: #f0f0f0;
    --bg-hover: #e8e8e8;
    --text: #333333;
    --text-muted: #999999;
    --accent: #7478ff;
}
.container { max-width: 1400px; margin: 0 auto; }
h1 { font-size: 22px; font-weight: 600; margin: 0 0 16px; }
.toolbar {
    display: flex;
    gap: 8px;
    margin-bottom: 20px;
}
.search {
    flex: 1;
    box-sizing: border-box;
    padding: 10px 14px;
    background: var(--bg-raised);
    border: 1px solid var(--bg-hover);
    border-radius: 6px;
    color: var(--text);
    font-size: 14px;
    font-family: inherit;
}
.search:focus { outline: none; border-color: var(--accent); }
.print-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 0 16px;
    background: var(--bg-raised);
    border: 1px solid var(--bg-hover);
    border-radius: 6px;
    color: var(--text);
    font-size: 14px;
    font-family: inherit;
    cursor: pointer;
    transition: background 0.1s, border-color 0.1s;
}
.print-btn:hover { background: var(--bg-hover); border-color: var(--accent); }
.print-btn:active { transform: translateY(1px); }
.print-btn svg { width: 16px; height: 16px; }
.columns {
    column-width: 340px;
    column-gap: 24px;
}
section {
    position: relative;
    break-inside: avoid;
    margin: 16px 0 22px;
    display: inline-block;
    width: 100%;
    box-sizing: border-box;
    border: 3px solid var(--cat);
    border-radius: 10px;
    padding: 12px 12px 6px;
}
h2 {
    position: absolute;
    top: -13px;
    left: 14px;
    margin: 0;
    padding: 3px 10px 4px;
    background: var(--cat);
    color: #ffffff;
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 1.4px;
    border-radius: 6px 6px 0 0;
    line-height: 1;
}
table { width: 100%; border-collapse: collapse; }
tr { border-bottom: 1px solid var(--bg-hover); }
tr:last-child { border-bottom: none; }
td { padding: 5px 4px; vertical-align: top; }
td.shortcut { width: 1%; white-space: nowrap; text-align: right; }
.name { font-size: 13px; }
.desc { font-size: 11px; color: var(--text-muted); margin-top: 1px; }
kbd {
    display: inline-block;
    padding: 3px 9px;
    background: var(--bg-raised);
    border: 1px solid var(--bg-hover);
    border-radius: 5px;
    font-family: 'Menlo', 'Consolas', monospace;
    font-size: 14px;
    font-weight: 600;
    color: var(--text);
    line-height: 1.2;
}
.scope {
    display: inline-block;
    margin-left: 6px;
    padding: 2px 7px;
    border-radius: 4px;
    font-size: 10px;
    font-weight: 600;
    color: var(--text-muted);
    background: var(--bg-raised);
    border: 1px solid var(--bg-hover);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    vertical-align: middle;
}
[hidden] { display: none !important; }
@media print {
    @page { margin: 12mm; }
    body {
        background: #ffffff !important;
        color: #000000 !important;
        padding: 0;
        font-size: 9pt;
        -webkit-print-color-adjust: exact;
        print-color-adjust: exact;
    }
    .container { max-width: none; }
    .columns { column-width: 240px; column-gap: 14px; }
    .toolbar { display: none !important; }
    h1 { color: #000000 !important; font-size: 14pt; margin-bottom: 8px; }
    section { padding: 10px 10px 4px; margin: 14px 0 18px; }
    .desc { color: #444444 !important; }
    tr { border-bottom-color: #cccccc !important; }
    kbd {
        background: #ffffff !important;
        border-color: #000000 !important;
        color: #000000 !important;
    }
    .scope {
        background: #ffffff !important;
        border-color: #888888 !important;
        color: #444444 !important;
    }
}
`;

const SCRIPT = `
const input = document.getElementById('search');
const sections = Array.from(document.querySelectorAll('section'));
input.addEventListener('input', () => {
    const q = input.value.trim().toLowerCase();
    for (const section of sections) {
        let any = false;
        for (const row of section.querySelectorAll('tr')) {
            const match = !q || row.dataset.search.includes(q);
            row.hidden = !match;
            if (match) any = true;
        }
        section.hidden = !any;
    }
});
document.getElementById('print').addEventListener('click', () => window.print());
input.focus();
`;

export function openCheatsheet() {
    const themeClass = theme.current === 'light' ? 'light' : 'dark';
    const html = `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Darkly Hotkeys</title><style>${STYLE}</style></head><body class="${themeClass}"><div class="container"><h1>Darkly Hotkey Cheat Sheet</h1><div class="toolbar"><input id="search" class="search" type="search" placeholder="Search shortcuts…" autocomplete="off" spellcheck="false"><button id="print" class="print-btn" type="button"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 6 2 18 2 18 9"/><path d="M6 18H4a2 2 0 0 1-2-2v-5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2h-2"/><rect x="6" y="14" width="12" height="8"/></svg>Print</button></div><div class="columns">${buildSections()}</div></div><script>${SCRIPT}<\/script></body></html>`;

    const win = window.open('', '_blank', 'width=900,height=1000');
    if (!win) return;
    win.document.open();
    win.document.write(html);
    win.document.close();
    win.focus();
}
