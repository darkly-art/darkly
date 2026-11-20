/*
 * The manual names keys by action, never by key: `:hotkey[id]` is the action's
 * key, `:action[id]` its display name in bold and then its key. The ids are the
 * Actions table's row ids.
 *
 * Rendered here, as gen-docs.mjs copies each page into the collection, rather
 * than by a remark plugin in Astro's pipeline, for two reasons. Astro's content
 * loader catches a render error, logs it and ships the page empty with the
 * build still passing, while a throw here fails the build before Astro starts.
 * And Astro re-renders an entry only when its file changes, so a plugin reading
 * the bindings from elsewhere would keep serving the old key after a preset
 * changed; spliced into the page, the key is part of the file.
 *
 * The page is parsed with the directive grammar only to locate the directives,
 * so code spans, fences and `\:` escapes stay literal; the markup is spliced
 * into the source text and every other byte reaches Astro as written. Text the
 * grammar reads as some other directive (`note:this`) is left alone.
 */
import { unified } from 'unified';
import remarkParse from 'remark-parse';
import remarkDirective from 'remark-directive';
import { esc, hotkeyHtml, kbdMarkdown } from '../src/lib/hotkeys.ts';

const UNBOUND = '<span class="hk-none" title="No key in this preset; bind one in Settings">no hotkey</span>';

const FORMS = {
    hotkey: (row, presets) => hotkeyHtml(row.chords, presets, { unbound: UNBOUND, key: kbdMarkdown }),
    // Unbound, the key is empty and the space after the name collapses into
    // the prose's own.
    action: (row, presets) => `<strong>${esc(row.name)}</strong> ${hotkeyHtml(row.chords, presets, { key: kbdMarkdown })}`,
};

const parser = unified().use(remarkParse).use(remarkDirective);
const FRONTMATTER = /^---\r?\n[\s\S]*?\r?\n---\r?\n/;
const text = (node) => node.value ?? (node.children ?? []).map(text).join('');

/**
 * Replace every `:hotkey[]` / `:action[]` in a manual page with its markup.
 * Throws, naming `where:line` and the id, on an unknown id or a directive in
 * a heading (its key would drop out of the heading's anchor and the contents).
 */
export function renderHotkeyDirectives(markdown, where, actions) {
    // Frontmatter is not markdown, and CommonMark would read it as a heading.
    const fm = FRONTMATTER.exec(markdown)?.[0] ?? '';
    const body = markdown.slice(fm.length);
    const lines = fm.split('\n').length - 1;
    const rows = new Map(actions.rows.map((r) => [r.id, r]));

    const found = [];
    const walk = (node, heading) => {
        for (const child of node.children ?? []) {
            if (child.type === 'textDirective' && Object.hasOwn(FORMS, child.name)) {
                const id = text(child);
                const at = `${where}:${child.position.start.line + lines}`;
                const row = rows.get(id);
                if (!row) throw new Error(`${at}: no action '${id}'`);
                if (heading) throw new Error(`${at}: :${child.name}[${id}] is in a heading, whose anchor and contents entry would lose the key`);
                let html;
                try {
                    html = FORMS[child.name](row, actions.presets);
                } catch (e) {
                    throw new Error(`${at}: :${child.name}[${id}]: ${e.message}`);
                }
                found.push({ start: child.position.start.offset, end: child.position.end.offset, html });
            } else {
                walk(child, heading || child.type === 'heading');
            }
        }
    };
    walk(parser.parse(body), false);

    let out = body;
    for (const { start, end, html } of found.reverse()) out = out.slice(0, start) + html + out.slice(end);
    return fm + out;
}
