#!/usr/bin/env node
/*
 * The built site works when served under BASE: every link resolves, and search
 * has an index covering every page and a bundle URL it can load.
 *
 * Over every page in dist/: a root-absolute href, src or poster must start
 * with BASE (the site home, `/`, is the one exception) and, with BASE, query
 * and hash stripped, name a file in dist/, a trailing slash meaning its
 * index.html. The search bundle's URL is built in script, not in an attribute,
 * so the scripts are checked for it separately.
 *
 * Every article has content after its <h1>: Astro logs a page that failed to
 * render, ships it empty and still exits 0. And every key is what `export-docs`
 * rendered: each key group indexes exactly its default variant, each generated
 * key's text is one of the export's chord texts (Astro's typography left it
 * alone), no directive survived as text, and none reaches Astro at all, since
 * Astro re-renders a page only when its file changes and would keep serving a
 * key the presets no longer bind.
 *
 *   node scripts/check-dist.mjs      (after `make docs`)
 */
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { BASE } from '../base.mjs';
import { OSES, chordText } from '../src/lib/hotkeys.ts';
import { renderHotkeyDirectives } from './hotkey-directives.mjs';

const DIST = fileURLToPath(new URL('../dist/', import.meta.url));
const CONTENT = fileURLToPath(new URL('../src/content/docs/', import.meta.url));
const actions = JSON.parse(fs.readFileSync(new URL('../src/generated/docs-actions.json', import.meta.url), 'utf8'));
const KEYS = new Set(actions.rows.flatMap((r) => actions.presets.flatMap((p) => OSES.map((os) => chordText(r.chords[p], os)))));
const NAMED = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'" };
const decode = (text) =>
    text
        .replace(/&#(x?)([0-9a-f]+);/gi, (_, hex, n) => String.fromCodePoint(parseInt(n, hex ? 16 : 10)))
        .replace(/&(amp|lt|gt|quot|apos);/g, (_, name) => NAMED[name]);

/** Each `<span class="hk">` group, through its matching close. */
function keyGroups(html) {
    const groups = [];
    for (const { index } of html.matchAll(/<span class="hk">/g)) {
        const tag = /<(\/?)span\b[^>]*>/g;
        tag.lastIndex = index;
        for (let depth = 0, t; (t = tag.exec(html)); ) {
            depth += t[1] ? -1 : 1;
            if (!depth) {
                groups.push(html.slice(index, tag.lastIndex));
                break;
            }
        }
    }
    return groups;
}
const ALLOWED = new Set(['/']);
const problems = [];

const files = fs.readdirSync(DIST, { recursive: true }).map((rel) => path.join(DIST, rel));
const pages = files.filter((f) => f.endsWith('.html'));
if (!pages.length) problems.push('dist/ has no pages; run `make docs` first');

let indexed = 0;
for (const page of pages) {
    const html = fs.readFileSync(page, 'utf8');
    const rel = path.relative(DIST, page);
    if (html.includes('data-pagefind-body')) indexed += 1;

    const article = /<article\b[^>]*>([\s\S]*?)<\/article>/.exec(html)?.[1];
    if (article !== undefined && !article.replace(/<h1\b[^>]*>[\s\S]*?<\/h1>/, '').trim()) {
        problems.push(`${rel}: the article is empty after its <h1>; Astro's log says why the page failed to render`);
    }
    for (const group of keyGroups(html)) {
        const indexedVariants = [...group.matchAll(/<span class="hk-v ([^"]*)"([^>]*)>/g)].filter(([, , attrs]) => !attrs.includes('data-pagefind-ignore'));
        if (indexedVariants.length !== 1 || !indexedVariants[0][1].split(' ').includes('hk-default')) {
            problems.push(`${rel}: a key group indexes ${indexedVariants.length} variant(s), not exactly its default: ${group.slice(0, 200)}`);
        }
    }
    for (const [, inner] of html.matchAll(/<kbd class="hk-key"[^>]*>([\s\S]*?)<\/kbd>/g)) {
        const text = decode(inner.replace(/<\/?code>/g, ''));
        if (!KEYS.has(text)) problems.push(`${rel}: key '${text}' is no chord text in the export; something rewrote it`);
    }
    if (/:(hotkey|action)\[/.test(html.replace(/<code\b[\s\S]*?<\/code>/g, ''))) {
        problems.push(`${rel}: a :hotkey[] or :action[] directive was left as text`);
    }
    for (const [, url] of html.matchAll(/\s(?:href|src|poster)="(\/(?!\/)[^"]*)"/g)) {
        if (ALLOWED.has(url)) continue;
        if (!url.startsWith(BASE)) {
            problems.push(`${rel}: ${url} is outside ${BASE}`);
            continue;
        }
        let target = decodeURIComponent(url.slice(BASE.length).split(/[?#]/)[0]);
        if (target === '' || target.endsWith('/')) target += 'index.html';
        if (!fs.existsSync(path.join(DIST, target))) problems.push(`${rel}: ${url} names no file`);
    }
}

for (const rel of fs.readdirSync(CONTENT, { recursive: true }).filter((f) => f.endsWith('.md'))) {
    const md = fs.readFileSync(path.join(CONTENT, rel), 'utf8');
    let rendered;
    try {
        rendered = renderHotkeyDirectives(md, rel, actions);
    } catch (e) {
        rendered = e.message;
    }
    if (rendered !== md) {
        problems.push(
            `src/content/docs/${rel}: a hotkey directive reaches Astro. gen-docs.mjs renders them so a page's ` +
                'text changes with its keys: Astro re-renders a page only when its file changes.',
        );
    }
}

const entry = path.join(DIST, 'pagefind', 'pagefind-entry.json');
if (!fs.existsSync(entry)) {
    problems.push('dist/pagefind/pagefind-entry.json is missing: no search index');
} else {
    const languages = Object.values(JSON.parse(fs.readFileSync(entry, 'utf8')).languages ?? {});
    const count = languages.reduce((sum, l) => sum + l.page_count, 0);
    if (count !== indexed) {
        problems.push(`search indexes ${count} page(s), but ${indexed} carry data-pagefind-body`);
    }
}

const scripts = files.filter((f) => f.endsWith('.js') && path.relative(DIST, f).startsWith('_astro'));
const loads = scripts.flatMap((f) => [...fs.readFileSync(f, 'utf8').matchAll(/(.{0,40})pagefind\/pagefind\.js/g)]);
if (!loads.length) problems.push('no script under dist/_astro/ loads pagefind/pagefind.js');
for (const [, before] of loads) {
    if (!before.endsWith(BASE)) problems.push(`search bundle loaded from '${before}pagefind/pagefind.js', not ${BASE}`);
}

if (problems.length) {
    console.error(problems.join('\n'));
    process.exit(1);
}
console.log(`dist/: ${pages.length} page(s) resolve under ${BASE}, ${indexed} indexed for search`);
