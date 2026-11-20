#!/usr/bin/env node
/*
 * The hotkey choice in a real browser, over the built site: what a reader with
 * no JavaScript sees, the convention detected from the browser, choosing a
 * preset in the header and having every key follow it across navigation with
 * no flash, a stored choice this build does not offer, the search index
 * holding only the default keys, and the header fitting at phone widths.
 *
 * Serves dist/ with `astro preview` and drives headless Chrome or Chromium
 * over the DevTools protocol with Node's own `fetch` and `WebSocket`, the
 * shape of frontend/scripts/bench-drive.mjs, which this project cannot import
 * since it reads nothing outside itself but the export. Each scenario runs in
 * its own browser context, so each starts with empty storage.
 *
 *   node scripts/check-browser.mjs      (after `make docs`; CHROME names the browser)
 */
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { BASE } from '../base.mjs';
import { ATTR, OSES, STORAGE_KEY, choiceToken, chordText, defaultPreset } from '../src/lib/hotkeys.ts';

const ROOT = fileURLToPath(new URL('..', import.meta.url));
const { presets, rows } = JSON.parse(fs.readFileSync(path.join(ROOT, 'src/generated/docs-actions.json'), 'utf8'));
const DEF = defaultPreset(presets);
const OTHER = presets.find((p) => p !== DEF);
const SITE = `http://127.0.0.1:4399${BASE}`;
const UA = {
    mac: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36',
    other: 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36',
};
// Every text a key can show in each choice.
const TEXTS = Object.fromEntries(
    presets.flatMap((p) => OSES.map((o) => [choiceToken(p, o), new Set(['', 'no hotkey', ...rows.map((r) => chordText(r.chords[p], o))])])),
);

const problems = [];
const check = (ok, msg) => ok || problems.push(msg);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function until(fn, what, ms = 20000) {
    for (const end = Date.now() + ms; Date.now() < end; await sleep(100)) {
        try {
            const v = await fn();
            if (v) return v;
        } catch {}
    }
    throw new Error(`timed out waiting for ${what}`);
}

const onPath = (bin) => path.isAbsolute(bin) ? fs.existsSync(bin) : (process.env.PATH ?? '').split(path.delimiter).some((d) => fs.existsSync(path.join(d, bin)));
const tried = process.env.CHROME ? [process.env.CHROME] : ['google-chrome', 'chromium'];
const browser = tried.find(onPath);
if (!browser) {
    console.error(`no browser found: tried ${tried.join(', ')}; set CHROME to one`);
    process.exit(1);
}

const children = [];
process.on('exit', () => children.forEach((c) => c.kill()));
children.push(spawn(path.join(ROOT, 'node_modules/.bin/astro'), ['preview', '--host', '127.0.0.1', '--port', '4399'], { cwd: ROOT, stdio: 'ignore' }));
// Ubuntu 24.04 restricts the unprivileged user namespaces Chrome's sandbox needs.
const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'darkly-docs-browser-'));
children.push(spawn(browser, ['--headless=new', '--no-sandbox', '--remote-debugging-port=0', `--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check', 'about:blank'], { stdio: 'ignore' }));

await until(async () => (await fetch(SITE)).ok, 'astro preview');
const port = await until(() => fs.readFileSync(path.join(profile, 'DevToolsActivePort'), 'utf8').split('\n')[0], 'the debugging port');
// The port answers before the browser has a page; until then the list can hold only an extension.
await until(async () => (await (await fetch(`http://127.0.0.1:${port}/json`)).json()).some((t) => t.type === 'page'), 'a page target');
const ws = new WebSocket((await (await fetch(`http://127.0.0.1:${port}/json/version`)).json()).webSocketDebuggerUrl);
await new Promise((resolve, reject) => Object.assign(ws, { onopen: resolve, onerror: reject }));

let seq = 0;
const pending = new Map();
const waiters = [];
ws.onmessage = ({ data }) => {
    const m = JSON.parse(data);
    const p = pending.get(m.id);
    if (p) {
        pending.delete(m.id);
        if (m.error) p.reject(new Error(`${p.method}: ${m.error.message}`));
        else p.resolve(m.result);
    }
    for (const w of waiters.filter((x) => x.method === m.method && x.session === m.sessionId)) {
        waiters.splice(waiters.indexOf(w), 1);
        w.resolve();
    }
};
const cdp = (method, params = {}, sessionId) =>
    new Promise((resolve, reject) => {
        pending.set(++seq, { resolve, reject, method });
        ws.send(JSON.stringify({ id: seq, method, params, sessionId }));
    });

/** A page in a fresh browser context: its own storage, user agent and width. */
async function tab({ os = 'other', scripts = true, width = 1024 } = {}) {
    const { browserContextId } = await cdp('Target.createBrowserContext');
    const { targetId } = await cdp('Target.createTarget', { url: 'about:blank', browserContextId });
    const { sessionId } = await cdp('Target.attachToTarget', { targetId, flatten: true });
    const s = (method, params) => cdp(method, params, sessionId);
    const loaded = (fn) => {
        const done = new Promise((resolve) => waiters.push({ method: 'Page.loadEventFired', session: sessionId, resolve }));
        return fn().then(() => done);
    };
    await s('Page.enable');
    await s('Emulation.setUserAgentOverride', { userAgent: UA[os] });
    await s('Emulation.setScriptExecutionDisabled', { value: !scripts });
    await s('Emulation.setDeviceMetricsOverride', { width, height: 800, deviceScaleFactor: 1, mobile: false });
    const t = {
        eval: async (expression) => {
            const r = await s('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
            if (r.exceptionDetails) throw new Error(`${expression.slice(0, 60)}: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
            return r.result.value;
        },
        go: (rel) => loaded(() => s('Page.navigate', { url: SITE + rel })),
        reload: () => loaded(() => s('Page.reload')),
        atStart: (source) => s('Page.addScriptToEvaluateOnNewDocument', { source }),
        attr: () => t.eval(`document.documentElement.getAttribute('${ATTR}')`),
        // Each key group's one visible variant, and every key outside a group;
        // preset labels are groups too, but hold no key.
        shown: () =>
            t.eval(`[...document.querySelectorAll('.docs-article .hk:has(.hk-key)')].map((g) => {
                const v = [...g.children].filter((c) => getComputedStyle(c).display !== 'none');
                return v.length === 1 ? [v[0].className, v[0].textContent] : ['', v.length + ' variants shown'];
            }).concat([...document.querySelectorAll('.docs-article kbd.hk-key')].filter((k) => !k.closest('.hk')).map((k) => [null, k.textContent]))`),
        close: () => cdp('Target.disposeBrowserContext', { browserContextId }),
    };
    return t;
}

/** Every key on the page shows `token`'s text. */
async function showsChoice(t, token, where) {
    const shown = await t.shown();
    check(shown.length > 0, `${where}: no keys on the page`);
    for (const [cls, text] of shown) {
        if (cls !== null) check(cls.split(' ').includes(`hk-${token}`), `${where}: shows '${text}' (${cls}), not ${token}'s`);
        check(TEXTS[token].has(text), `${where}: '${text}' is not one of ${token}'s keys`);
    }
    return shown;
}
const header = (t) => t.eval(`[...document.querySelectorAll('.hk-toggle-label .hk-v')].find((v) => getComputedStyle(v).display !== 'none')?.textContent`);

try {
    // 1. Scripts off: the default preset, Windows / Linux, and no control.
    const off = await tab({ scripts: false });
    for (const rel of ['', 'reference/actions/']) {
        await off.go(rel);
        check((await off.attr()) === null, `no-JS ${rel}: data-hotkeys is set`);
        await showsChoice(off, choiceToken(DEF, 'other'), `no-JS ${rel}`);
        check((await off.eval(`getComputedStyle(document.querySelector('.hk-toggle')).display`)) === 'none', `no-JS ${rel}: the control shows`);
    }
    await off.close();

    // 2. Before any choice: the convention follows the browser.
    const mac = await tab({ os: 'mac' });
    await mac.go('reference/actions/');
    check((await mac.attr()) === choiceToken(DEF, 'mac'), `macOS: data-hotkeys is ${await mac.attr()}`);
    const macKeys = (await showsChoice(mac, choiceToken(DEF, 'mac'), 'macOS')).map(([, x]) => x).join(' ');
    check(/⌘/.test(macKeys) && !/\b(Ctrl|Alt|Shift)\b/.test(macKeys), `macOS: keys read '${macKeys.slice(0, 80)}'`);
    await mac.close();

    const t = await tab();
    await t.go('reference/actions/');
    const before = await showsChoice(t, choiceToken(DEF, 'other'), 'Linux');
    check(!/[⌘⌥⇧]/.test(before.map(([, x]) => x).join(' ')), 'Linux: keys use Apple glyphs');

    // 3. Choosing a preset in the header: storage, every key, the table's count line and filter.
    const token = choiceToken(OTHER, 'other');
    const pick = rows.find((r) => {
        const key = chordText(r.chords[OTHER], 'other').toLowerCase();
        const rest = `${r.name} ${r.description} ${r.category} ${r.id} ${chordText(r.chords[DEF], 'other')}`.toLowerCase();
        return key && !rest.includes(key);
    });
    const filter = (q) =>
        t.eval(`(async () => { const i = document.querySelector('.not-content input'); i.value = ${JSON.stringify(q)};
            i.dispatchEvent(new Event('input')); await new Promise((r) => setTimeout(r, 100));
            return [...document.querySelectorAll('.not-content .name strong')].map((s) => s.textContent); })()`);
    await until(() => t.eval(`!document.querySelector('astro-island[ssr]')`), 'the table to hydrate');
    const query = chordText(pick.chords[OTHER], 'other');
    check(!(await filter(query)).includes(pick.name), `${DEF}: filtering '${query}' shows ${pick.name}`);
    await filter('');
    await t.eval(`document.querySelector('.hk-toggle').click(); document.querySelector('input[name="hk-preset"][value="${OTHER}"]').click()`);
    check((await t.eval(`localStorage.getItem('${STORAGE_KEY}')`)) === token, `chose ${OTHER}: storage holds ${await t.eval(`localStorage.getItem('${STORAGE_KEY}')`)}`);
    const after = await showsChoice(t, token, `chose ${OTHER}`);
    check(after.some(([, x], i) => x !== before[i][1]), `chose ${OTHER}: no key on the page changed`);
    check((await t.eval(`document.querySelector('.count .hk-${token}')?.checkVisibility()`)) === true, `chose ${OTHER}: the count line does not name it`);
    check((await header(t)) === OTHER, `chose ${OTHER}: the header reads ${await header(t)}`);
    check((await filter(query)).includes(pick.name), `chose ${OTHER}: filtering '${query}' misses ${pick.name}`);

    // 4. Navigation and reload: the choice holds, and is set before the header and any key exist.
    await t.atStart(`new MutationObserver((ms) => { for (const m of ms) for (const n of m.addedNodes)
        if (n.nodeType === 1 && n.matches('.docs-header, .hk, kbd.hk-key')) (window.__seen ??= []).push(document.documentElement.getAttribute('${ATTR}'));
    }).observe(document, { childList: true, subtree: true });`);
    for (const step of [() => t.go(''), () => t.reload()]) {
        await step();
        const seen = await t.eval('window.__seen ?? []');
        check(seen.length > 1 && seen.every((v) => v === token), `after navigating: data-hotkeys as elements arrived was ${JSON.stringify(seen.slice(0, 4))}`);
        await showsChoice(t, token, 'after navigating');
    }

    // 5. A stored choice this build does not offer falls back to the default.
    await t.eval(`localStorage.setItem('${STORAGE_KEY}', 'Procreate-mac')`);
    await t.reload();
    check((await t.attr()) === choiceToken(DEF, 'other'), `unknown stored choice: data-hotkeys is ${await t.attr()}`);

    // 6. Search holds the default variant only.
    const variants = await t.eval(`[...document.querySelector('.docs-article .hk').children].map((v) => [v.classList.contains('hk-default'), v.textContent])`);
    const term = variants.find(([d]) => d)[1].split(' / ')[0];
    const hits = await t.eval(`(async () => { const pf = await import('${BASE}pagefind/pagefind.js');
        const r = await pf.search(${JSON.stringify(term)}); return Promise.all(r.results.map(async (x) => { const d = await x.data(); return [d.url, d.excerpt]; })); })()`);
    const excerpt = (hits.find(([url]) => url === BASE)?.[1] ?? '').replace(/<[^>]+>/g, '');
    check(excerpt.includes(term), `search '${term}': no excerpt from ${BASE} holds it (${JSON.stringify(hits)})`);
    for (const [d, text] of variants) if (!d && text) check(!excerpt.includes(text), `search '${term}': the excerpt holds '${text}'`);
    await t.close();

    // 7. The header fits: the control ends the search row on phones, and sits on the wordmark's row on desktop.
    for (const width of [320, 360, 1024]) {
        const w = await tab({ width });
        await w.go('');
        const [scroll, client, hk, search, brand] = await w.eval(`[document.documentElement.scrollWidth, document.documentElement.clientWidth,
            ...['.docs-hotkeys-area', '.docs-search-area', '.docs-brand'].map((s) => { const r = document.querySelector(s).getBoundingClientRect(); return r.top + r.height / 2; })]`);
        check(scroll === client, `${width}px: the page scrolls sideways (${scroll} > ${client})`);
        check(Math.abs(hk - (width < 800 ? search : brand)) < 4, `${width}px: the control is not on the ${width < 800 ? 'search' : "wordmark's"} row`);
        await w.close();
    }
} catch (e) {
    problems.push(e.stack ?? String(e));
} finally {
    ws.close();
    children.forEach((c) => c.kill());
    await sleep(500);
    fs.rmSync(profile, { recursive: true, force: true, maxRetries: 5 });
}

if (problems.length) {
    console.error(problems.join('\n'));
    process.exit(1);
}
console.log(`browser: hotkeys follow the choice across ${presets.length} presets, with no JavaScript, in search and at 320 to 1024 px`);
