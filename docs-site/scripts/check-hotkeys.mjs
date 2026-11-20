#!/usr/bin/env node
/*
 * Hotkey markup and the manual's directives, over a small fixture: no build,
 * no export.
 *
 * What a built page does with this markup (Astro's markdown pipeline, the
 * variant a reader sees) is check-dist.mjs's and check-browser.mjs's.
 *
 *   node scripts/check-hotkeys.mjs
 */
import assert from 'node:assert/strict';
import {
    choiceToken,
    choices,
    chordText,
    hotkeyHtml,
    kbdHtml,
    kbdMarkdown,
    presetLabelHtml,
    variantCss,
} from '../src/lib/hotkeys.ts';
import { renderHotkeyDirectives } from './hotkey-directives.mjs';

const presets = ['GIMP', 'Krita', 'Photoshop'];
const alt = { mac: '⌥+click', other: 'Alt+click' };
const all = (chords) => Object.fromEntries(presets.map((p) => [p, chords]));
const rows = [
    // Krita and Photoshop resolve the same chord twice; GIMP binds nothing.
    { id: 'isolateLayer', name: 'Isolate Layer', chords: { Krita: [alt, alt], Photoshop: [alt, alt] } },
    {
        id: 'commandPalette',
        name: 'Command Palette',
        chords: all([{ mac: '⌘+⇧+P', other: 'Ctrl+Shift+P' }, { mac: '⌘+F', other: 'Ctrl+F' }]),
    },
    { id: 'brushTool', name: 'Brush', chords: all([{ mac: 'B', other: 'B' }]) },
    { id: 'nothing', name: "Nobody's *Action*", chords: {} },
    { id: 'pipe', name: 'Pipe', chords: { GIMP: [{ mac: 'a|b', other: 'a|b' }] } },
];
const actions = { presets, rows };
const row = (id) => rows.find((r) => r.id === id);

/** `[classes, pagefind-ignored, inner html]` for each variant span. */
const variants = (html) =>
    [...html.matchAll(/<span class="hk-v ([^"]*)"( data-pagefind-ignore)?>(.*?)<\/span>(?=<span class="hk-v|<\/span>$)/g)].map(
        ([, cls, ignored, inner]) => [cls.split(' '), Boolean(ignored), inner],
    );

const tests = {
    'chordText drops a repeated chord and joins several': () => {
        assert.equal(chordText(row('isolateLayer').chords.Krita, 'other'), 'Alt+click');
        assert.equal(chordText(row('commandPalette').chords.GIMP, 'mac'), '⌘+⇧+P / ⌘+F');
        assert.equal(chordText(undefined, 'other'), '');
    },

    'one text in every combination is a single key': () => {
        assert.equal(hotkeyHtml(row('brushTool').chords, presets), kbdHtml('B'));
    },

    'several texts: one default variant, every token once, unbound filled in': () => {
        const html = hotkeyHtml(row('isolateLayer').chords, presets, { unbound: 'NONE' });
        const found = variants(html);
        assert.equal(found.length, 3);
        const defaults = found.filter(([cls]) => cls.includes('hk-default'));
        assert.equal(defaults.length, 1);
        const [cls, ignored, inner] = defaults[0];
        assert.ok(cls.includes(`hk-${choiceToken('Photoshop', 'other')}`));
        assert.equal(inner, kbdHtml('Alt+click'));
        assert.equal(ignored, false);
        for (const [c, i] of found) if (!c.includes('hk-default')) assert.equal(i, true);
        const tokens = found.flatMap(([c]) => c.filter((x) => x.startsWith('hk-') && x !== 'hk-v' && x !== 'hk-default'));
        assert.deepEqual(tokens.sort(), choices(presets).map((c) => `hk-${c.token}`).sort());
        assert.equal(found.find(([c]) => c.includes('hk-GIMP-mac'))[2], 'NONE');
    },

    'markup is one line in either key form': () => {
        for (const r of rows.filter((x) => x.id !== 'pipe')) {
            for (const key of [kbdHtml, kbdMarkdown]) assert.ok(!hotkeyHtml(r.chords, presets, { key }).includes('\n'), r.id);
        }
        assert.ok(!presetLabelHtml(presets).includes('\n'));
    },

    'kbdHtml escapes markdown and HTML syntax': () => {
        assert.equal(kbdHtml('*_[]<'), '<kbd class="hk-key">&#42;&#95;&#91;&#93;&#60;</kbd>');
    },

    'kbdMarkdown is a code span, fenced past any backtick': () => {
        assert.equal(kbdMarkdown("Ctrl+'"), "<kbd class=\"hk-key\">`Ctrl+'`</kbd>");
        assert.equal(kbdMarkdown('Ctrl+`'), '<kbd class="hk-key">`` Ctrl+` ``</kbd>');
        assert.throws(() => kbdMarkdown('a|b'), /'\|'/);
    },

    'a preset name with a space is a valid token': () => {
        assert.match(choiceToken('Clip Studio', 'mac'), /^[A-Za-z0-9_]+-mac$/);
    },

    'variantCss: one rule per token plus the default': () => {
        const css = variantCss(presets);
        for (const { token } of choices(presets)) {
            assert.equal(css.split(`html[data-hotkeys="${token}"] .hk-${token}{display:inline}`).length, 2, token);
        }
        assert.ok(css.includes('html:not([data-hotkeys]) .hk-default{display:inline}'));
        assert.equal(css.trim().split('\n').length, choices(presets).length + 2);
    },

    'directives render; everything else is left as written': () => {
        const literal = 'Keep note:this, `:hotkey[undo]` and \\:hotkey[undo].';
        const page = `---\ntitle: X\n---\n\nPress :hotkey[commandPalette] now.\n\n- :action[isolateLayer] isolates.\n\n${literal}\n`;
        const out = renderHotkeyDirectives(page, 'manual/x.md', actions);
        assert.ok(out.startsWith('---\ntitle: X\n---\n\nPress <span class="hk">'), out);
        assert.ok(out.includes(kbdMarkdown('Ctrl+Shift+P / Ctrl+F')));
        assert.ok(out.includes('- <strong>Isolate Layer</strong> <span class="hk">'));
        assert.ok(out.endsWith(`</span> isolates.\n\n${literal}\n`));
        assert.ok(!/:(hotkey|action)\[(commandPalette|isolateLayer)\]/.test(out));
        const plain = `---\ntitle: X\n---\n\n${literal}\n`;
        assert.equal(renderHotkeyDirectives(plain, 'manual/x.md', actions), plain);
    },

    'unbound: a chip for :hotkey, the name alone for :action': () => {
        assert.match(renderHotkeyDirectives(':hotkey[nothing]', 'manual/x.md', actions), /^<span class="hk-none"[^>]*>no hotkey<\/span>$/);
        assert.equal(renderHotkeyDirectives(':action[nothing]', 'manual/x.md', actions), '<strong>Nobody&#39;s &#42;Action&#42;</strong> ');
    },

    'unknown id, a heading, a pipe: each fails naming file and line': () => {
        const render = (md) => () => renderHotkeyDirectives(md, 'manual/x.md', actions);
        assert.throws(render('a\n\nSee :hotkey[nope].\n'), { message: "manual/x.md:3: no action 'nope'" });
        assert.throws(render('---\ntitle: X\n---\n\n## Search with :hotkey[commandPalette]\n'), /^Error: manual\/x\.md:5: .*heading/);
        assert.throws(render('See :hotkey[pipe].'), /^Error: manual\/x\.md:1: .*'\|'/);
    },
};

let failed = 0;
for (const [name, test] of Object.entries(tests)) {
    try {
        test();
    } catch (e) {
        failed += 1;
        console.error(`FAIL ${name}\n     ${String(e.message).split('\n').join('\n     ')}`);
    }
}
if (failed) {
    console.error(`${failed} of ${Object.keys(tests).length} hotkey check(s) failed`);
    process.exit(1);
}
console.log(`hotkeys: ${Object.keys(tests).length} checks pass`);
