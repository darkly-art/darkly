#!/usr/bin/env node
/*
 * Turn the docs export into pages of the `docs` collection.
 *
 * The export is the directory `DOCS_EXPORT` names (see "Reading the export"
 * below). Everything under `src/content/docs/` and `src/generated/` is output
 * of this script and is gitignored: the prose is copied from the export's
 * `manual/`, the reference pages are generated from its `metadata.json`, and
 * the pictures they embed are copied out of its `previews/` and `graphics/`.
 *
 * The layout this builds was specified up front:
 *
 *   tools              a page per tool-ish catalog, brushes front and centre
 *   actions & hotkeys  one searchable table, a column per preset
 *   layer types        a separate sub-page and table per layer type
 *
 * Reference pages are **static markdown tables**. They need no JavaScript, and
 * the Pagefind index then covers every row, so searching the docs for an effect
 * finds the effect. The actions table is the one exception, because "searchable"
 * was in the requirement, so it ships as a Svelte island.
 *
 *   DOCS_EXPORT=<dir> node scripts/gen-docs.mjs
 */

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { BASE } from '../base.mjs';
import { variantCss } from '../src/lib/hotkeys.ts';
import { renderHotkeyDirectives } from './hotkey-directives.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const OUT = path.join(ROOT, 'src', 'content', 'docs');
const DATA = path.join(ROOT, 'src', 'generated');

/*
 * Preview media is copied out of the export into `public/`, which Astro serves
 * verbatim under the base. The alternative, `src/assets/`, wants a static
 * import per file and these are named by a JSON index at build time.
 */
const MEDIA = path.join(ROOT, 'public', 'previews');
const MEDIA_URL = `${BASE}previews`;

/*
 * Catalog graphics the same way: the committed cards under
 * `docs/images/graphics/` (one per brush pack, rendered by
 * `frontend/scripts/render-doc-graphics.mjs`), which the export carries as
 * `graphics/` with the `.hash.json` sidecar each was rendered with.
 */
const GRAPHICS = path.join(ROOT, 'public', 'graphics');
const GRAPHICS_URL = `${BASE}graphics`;

/*
 * Which catalogs make up which section, and in what order they appear in the
 * sidebar. A catalog absent from here is simply not published, and stays in the
 * export for whenever we want it: the 13 `settings.*` preference groups,
 * which the specified layout has no section for, and `layerFilters`.
 *
 * `veils` and `filters` were separate catalogs until v0.8.0, when Darkly merged
 * them into `effects` and kept the old split as the entries' `category`. They
 * are one page here for the same reason: the distinction is a grouping inside a
 * table of things that all do the one thing the catalog describes.
 *
 * `layerKinds` is withheld too: as a page inside the Layer types section it read
 * as a duplicate of the section itself, one list of layer sorts nested under
 * another. What a layer fundamentally *is* belongs in the manual, not in a
 * reference table beside the effects you stack on it.
 *
 * **Brushes is a section, not the first page of Tools.** The specified layout
 * put it inside tools ("beginning with paint brush front and center"), which
 * was about prominence, and being the first row of another section is not
 * prominence. It leads the reference for the same reason it led that group, and
 * `brushNodes` follows it because a brush graph's nodes are what a brush is made
 * of; that leaves Tools as what the word means in the editor, the twelve things
 * on the toolbar. `brushPacks` opens the section: a pack is how the brushes are
 * met in the editor, and its page is the cards rather than a table (see
 * `groupPage`), so it is the one reference page that is a picture of what it
 * describes.
 *
 * A section holding one catalog is written as a page rather than a directory
 * (`reference/tools.md`, not `reference/tools/tools.md`), because a group whose
 * single child repeats its own name is the `layerKinds` mistake again, and the
 * sidebar already has a shape for a one-page section.
 */
const SECTIONS = {
    brushes: ['brushPacks', 'brushes', 'brushNodes'],
    'layer-types': ['effects', 'voids', 'blendModes'],
    tools: ['tools'],
};

/** Catalogs whose rows belong in the actions table, joined by `hotkeyAction`. */
const ACTION_SOURCES = ['actions', 'tools', 'effects'];

/*
 * The one line that says what the page is. It is the only prose on any generated
 * page not written by Darkly itself, so it matches how the catalog descriptions
 * read: says what you are looking at, once, and stops.
 */
const ACTIONS_INTRO =
    'Every action in Darkly and its keyboard shortcut.';

const log = (msg) => console.log(`[docs] ${msg}`);

function fail(msg) {
    console.error(`\n[docs] ${msg}\n`);
    process.exit(1);
}

// ---------------------------------------------------------------------------
// Reading the export
// ---------------------------------------------------------------------------

/*
 * Everything this site is built from sits in one directory, named by
 * `DOCS_EXPORT`: `metadata.json` (every registry), `icons.json` (each icon name
 * resolved to SVG), `manual/` (the hand-written pages) and, unless previews
 * were skipped, `assets.json` plus `previews/`. Nothing outside it and this
 * project is read, so the site builds from any directory of that shape.
 */
function source() {
    const dir = process.env.DOCS_EXPORT;
    if (!dir) fail('DOCS_EXPORT is unset; it names the export directory to build from');
    for (const required of ['metadata.json', 'icons.json', 'manual']) {
        if (!fs.existsSync(path.join(dir, required))) {
            fail(`DOCS_EXPORT (${dir}) has no ${required}`);
        }
    }
    const read = (name) => JSON.parse(fs.readFileSync(path.join(dir, name), 'utf8'));
    const metadata = read('metadata.json');
    const icons = read('icons.json');

    /*
     * Previews need a GPU device and ffmpeg, so an export can legitimately
     * arrive without them (`DOCS_PREVIEWS=0`). Tables then fall back to the
     * icon column rather than failing a build over an asset nothing else
     * depends on.
     */
    let assets = {};
    if (fs.existsSync(path.join(dir, 'assets.json'))) {
        assets = read('assets.json').assets ?? {};
    } else {
        console.warn('[docs] export carries no assets.json; tables will omit previews');
    }

    return { dir, metadata, icons, assets };
}

/**
 * The catalog graphics the export carries, by file stem, each with the size its
 * sidecar recorded. Absent when the export has no `graphics/`: a build from an
 * export made without them still gets every table, and a page that needs a
 * card fails by name in `groupPage` rather than shipping a broken image.
 */
function readGraphics(dir) {
    const from = path.join(dir, 'graphics');
    const graphics = {};
    if (!fs.existsSync(from)) return graphics;
    for (const file of fs.readdirSync(from)) {
        if (!file.endsWith('.hash.json')) continue;
        const name = file.replace(/\.hash\.json$/, '');
        const { width, height } = JSON.parse(fs.readFileSync(path.join(from, file), 'utf8'));
        graphics[name] = { file: `${name}.jpg`, width, height };
    }
    return graphics;
}

/**
 * Copy the export's rendered previews into `public/`.
 *
 * Wholesale rather than per referenced entry: the tree is what the encoder
 * decided to ship, and a file in it that no row names is a bug to see, not a
 * file to quietly drop. Wiped first so a withdrawn preview stops being served.
 */
function copyPreviews(dir) {
    fs.rmSync(MEDIA, { recursive: true, force: true });
    const from = path.join(dir, 'previews');
    if (!fs.existsSync(from)) return 0;

    let count = 0;
    const walk = (src, dest) => {
        fs.mkdirSync(dest, { recursive: true });
        for (const entry of fs.readdirSync(src, { withFileTypes: true })) {
            if (entry.isDirectory()) walk(path.join(src, entry.name), path.join(dest, entry.name));
            else {
                fs.copyFileSync(path.join(src, entry.name), path.join(dest, entry.name));
                count += 1;
            }
        }
    };
    walk(from, MEDIA);
    return count;
}

/** Copy the export's catalog graphics into `public/`, images only: the
 *  sidecars were read for their sizes and are nothing to serve. */
function copyGraphics(dir, graphics) {
    fs.rmSync(GRAPHICS, { recursive: true, force: true });
    const names = Object.keys(graphics);
    if (!names.length) return 0;
    fs.mkdirSync(GRAPHICS, { recursive: true });
    for (const name of names) {
        fs.copyFileSync(path.join(dir, 'graphics', graphics[name].file), path.join(GRAPHICS, graphics[name].file));
    }
    return names.length;
}

// ---------------------------------------------------------------------------
// Markdown emission
// ---------------------------------------------------------------------------

/**
 * A markdown table cell is terminated by a newline and split by a pipe, so both
 * have to go. Descriptions are prose written for the UI and contain neither by
 * habit, but nothing in the registries enforces that.
 */
function cell(text) {
    return String(text ?? '')
        .replace(/\s+/g, ' ')
        .replace(/\|/g, '\\|')
        .trim();
}

/**
 * Frontmatter is the contract the `docs` collection reads: title, description,
 * position within its sidebar group.
 *
 * Nothing here suppresses the table of contents: a reference page is an intro
 * line and one table, so it has no headings, and TableOfContents.astro renders
 * nothing when there are fewer than two. Hand-written manual prose does have
 * headings and keeps its rail, with no frontmatter needed either way.
 */
function frontmatter({ title, description, order }) {
    const lines = ['---', `title: ${JSON.stringify(title)}`];
    if (description) lines.push(`description: ${JSON.stringify(description)}`);
    if (order !== undefined) lines.push(`order: ${order}`);
    lines.push('---', '');
    return lines.join('\n');
}

let iconSeq = 0;

/**
 * Inline SVG for one Iconify name, forced onto a single line because it has to
 * survive inside a table cell.
 *
 * Ids are rewritten per occurrence. Iconify's runtime uniquifies the ids inside
 * an icon body precisely because inlining the same icon twice would otherwise
 * collide; static inlining skips that step, so it happens here instead. No icon
 * Darkly currently names carries one; this is what keeps that from becoming a
 * silent rendering bug the day one does.
 */
function iconSvg(icons, name, label) {
    const icon = icons[name];
    if (!icon) return '';

    let body = icon.body.replace(/\s+/g, ' ').trim();
    const ids = [...body.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]);
    if (ids.length) {
        const suffix = `d${(iconSeq += 1)}`;
        for (const id of ids) {
            const esc = id.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
            body = body
                .replace(new RegExp(`\\bid="${esc}"`, 'g'), `id="${id}-${suffix}"`)
                .replace(new RegExp(`url\\(#${esc}\\)`, 'g'), `url(#${id}-${suffix})`);
        }
    }

    const viewBox = `${icon.left} ${icon.top} ${icon.width} ${icon.height}`;
    return (
        `<svg class="docs-icon" viewBox="${viewBox}" width="20" height="20" ` +
        `role="img" aria-label="${cell(label)}">${body}</svg>`
    );
}

/**
 * One rendered preview, as a table cell.
 *
 * A still at rest and motion on demand, the same answer the editor's own
 * pickers give, and the reason the encoder's index nominates a poster frame at
 * all. `preload="none"` is what makes that affordable: sixteen blend modes on
 * one page load sixteen ~3 KB posters and no video until something is hovered.
 * Without JavaScript the poster is simply what the element shows, which is why
 * the markup is a `<video>` with a poster rather than an `<img>` a script has to
 * swap.
 *
 * A single-frame entry has no `video` (every brush is one) and gets an `<img>`
 * instead, because a `<video>` that can never move is a promise the page does
 * not keep.
 */
function previewMedia(asset, label, version) {
    // Paths in the index are export-relative (`previews/effects/grain.webp`) and
    // that directory's *contents* were copied to MEDIA_URL, so the leading
    // segment is the one thing that does not survive the move.
    const url = (rel) => `${MEDIA_URL}/${rel.replace(/^previews\//, '')}?v=${encodeURIComponent(version)}`;
    // The attributes reserve the row's height before a byte of media arrives.
    // The inline ratio is for `<video>`: an image derives one from these two
    // attributes by itself, but a video with `preload="none"` has loaded no
    // metadata and falls back to the 2:1 default box until it does, which
    // letterboxes a square preview and reflows the row when it finally plays.
    const box =
        `width="${asset.width}" height="${asset.height}" ` +
        `style="aspect-ratio: ${asset.width} / ${asset.height}"`;

    if (!asset.video) {
        return (
            `<img class="docs-preview" src="${url(asset.poster)}" ${box} alt="" ` +
            `loading="lazy" decoding="async">`
        );
    }
    return (
        `<video class="docs-preview" src="${url(asset.video)}" poster="${url(asset.poster)}" ` +
        `${box} muted playsinline preload="none"${asset.loop ? ' loop' : ''} ` +
        `aria-label="${cell(label)} preview"></video>`
    );
}

/**
 * One catalog as a page. Columns are chosen from what the catalog actually
 * carries: some catalogs declare no icons (they show a live preview instead of
 * one) and most do not group, so neither gets an empty column.
 */
function catalogPage(catalog, icons, assets, order, version) {
    /*
     * One visual column, not two.
     *
     * An icon and a preview answer the same question (what does this look like),
     * and every catalog that renders previews is one the editor gives a live
     * thumbnail instead of a glyph. So a cell shows the entry's preview if it
     * has one and its icon otherwise, and `effects` (14 icons, 14 previews) shows
     * previews rather than both. `voids` is the mixed case the fallback exists
     * for: one renders, the others declare no preview and keep glyphs.
     *
     * A null icon is a deliberate declaration, not an omission, so a partly
     * filled column is honest; the question is only whether it earns its width.
     * Majority carries it, except that any preview at all carries it, because a
     * rendered asset is the one thing a reference table cannot say in words.
     */
    const preview = (e) => assets[catalog.id]?.[e.type];
    const withPreviews = catalog.entries.filter(preview).length;
    const withIcons = catalog.entries.filter((e) => e.icon && icons[e.icon]).length;
    const hasVisual = withPreviews > 0 || (withIcons > 0 && withIcons * 2 >= catalog.entries.length);
    const hasCategory = catalog.entries.some((e) => e.category);

    const visual = (e) => {
        const asset = preview(e);
        if (asset) return previewMedia(asset, e.displayName, version);
        return iconSvg(icons, e.icon, e.displayName);
    };

    const head = [...(hasVisual ? [''] : []), 'Name', ...(hasCategory ? ['Category'] : []), 'Description'];
    const rows = [...catalog.entries]
        .sort((a, b) =>
            (a.category ?? '').localeCompare(b.category ?? '') ||
            a.displayName.localeCompare(b.displayName),
        )
        .map((e) =>
            [
                ...(hasVisual ? [visual(e)] : []),
                `**${cell(e.displayName)}**`,
                ...(hasCategory ? [cell(e.category)] : []),
                cell(e.description),
            ].join(' | '),
        );

    return [
        frontmatter({ title: catalog.title, description: catalog.description, order }),
        catalog.description ? `${catalog.description}\n` : '',
        `| ${head.join(' | ')} |`,
        `|${head.map(() => '---').join('|')}|`,
        ...rows.map((r) => `| ${r} |`),
        '',
        // Footers say what the page covers and how current it is. They do not
        // mention registries, catalogs or ids; a painter reading this has no
        // reason to know Darkly has any of those.
        `*All ${catalog.entries.length} ${catalog.title.toLowerCase()} in Darkly ${version}.*`,
        '',
    ].join('\n');
}

/**
 * A catalog of groups as a page: a card per entry rather than a row.
 *
 * Each card already carries the pack's icon, name, description and every
 * member's stroke in the pack's own colours, so the page is the cards and
 * nothing beside them: a heading or a caption would say what the picture
 * says. The words go in the alt text instead, where a screen reader and the
 * search index find them without the page repeating itself.
 *
 * The card is the committed graphic for `<catalog>-<type>`, the stem
 * `render-doc-graphics.mjs` writes and the README links. A group without one
 * fails the build by name: a page with a hole where a picture should be is
 * exactly what nobody notices before deploy.
 */
function groupPage(catalog, metadata, graphics, order, version) {
    const nameOf = (members, id) =>
        metadata.catalogs.find((c) => c.id === members.catalog)?.entries.find((e) => e.type === id)?.displayName ?? id;
    const cards = catalog.entries.map((e) => {
        const name = `${catalog.id}-${e.type}`;
        const card = graphics[name];
        if (!card) {
            fail(
                `no card for ${catalog.id}/${e.type}: the export has no graphics/${name}.jpg\n` +
                    '       render it: node frontend/scripts/render-doc-graphics.mjs --metadata <export>/metadata.json',
            );
        }
        const members = e.members.entries.map((id) => nameOf(e.members, id)).join(', ');
        // Each part a sentence, so a description that already ends in a
        // full stop does not get a second one.
        const alt = [e.displayName, e.description, members]
            .filter(Boolean)
            .map((part) => part.replace(/\.?$/, '.'))
            .join(' ');
        return (
            `<img class="docs-card" src="${GRAPHICS_URL}/${card.file}?v=${encodeURIComponent(version)}" ` +
            `width="${card.width}" height="${card.height}" alt="${cell(alt)}" loading="lazy" decoding="async">`
        );
    });
    return [
        frontmatter({ title: catalog.title, description: catalog.description, order }),
        catalog.description ? `${catalog.description}\n` : '',
        ...cards.map((c) => `${c}\n`),
        `*All ${catalog.entries.length} ${catalog.title.toLowerCase()} in Darkly ${version}.*`,
        '',
    ].join('\n');
}

/** A catalog whose every entry groups entries of another is a page of cards;
 *  anything else is a table. Decided by what the entries carry, so a second
 *  grouping catalog would take the same shape with no change here. */
function isGroupCatalog(catalog) {
    return catalog.entries.length > 0 && catalog.entries.every((e) => e.members);
}

// ---------------------------------------------------------------------------
// The actions island's data
// ---------------------------------------------------------------------------

/**
 * Every action, joined to its chords.
 *
 * The join rule is `export-docs`' own: a row is a catalog entry carrying a
 * `hotkeyAction`, and its chords are `bindings[preset][thatAction]`. Tools and
 * filters register their hotkey metadata on their own entries rather than
 * duplicating it into the actions registry, so all three catalogs feed one
 * table and nothing has to know which one a row came from.
 */
function actionsData(metadata, icons) {
    const rows = [];
    const used = new Set();

    for (const id of ACTION_SOURCES) {
        const catalog = metadata.catalogs.find((c) => c.id === id);
        if (!catalog) continue;
        for (const entry of catalog.entries) {
            if (!entry.hotkeyAction || used.has(entry.hotkeyAction)) continue;
            used.add(entry.hotkeyAction);
            rows.push({
                id: entry.hotkeyAction,
                name: entry.displayName,
                description: entry.description ?? '',
                category: entry.category ?? catalog.title,
                // Resolved here rather than in the island: the id-uniquifying
                // and single-lining lives in one place, and the component stays
                // a table instead of also being an icon renderer.
                iconSvg: entry.icon ? iconSvg(icons, entry.icon, entry.displayName) : '',
                source: catalog.id,
            });
        }
    }

    /*
     * The editor presets only. `defaults` is the editor-agnostic baseline they
     * layer over, and `export-docs` writes each preset already resolved through
     * it: GIMP carries 45 bindings, Krita 47, Photoshop 48, baseline included.
     * So offering it as a fourth choice would show a reader a keymap nobody
     * actually paints with, and every binding in it appears under the presets
     * anyway.
     */
    const presets = Object.keys(metadata.bindings)
        .filter((p) => p !== 'defaults')
        .sort((a, b) => a.localeCompare(b));

    for (const row of rows) {
        row.chords = {};
        for (const preset of presets) {
            const found = metadata.bindings[preset]?.[row.id];
            if (found?.length) {
                row.chords[preset] = found.map((b) => ({ mac: b.mac, other: b.other }));
            }
        }
    }

    rows.sort((a, b) => a.category.localeCompare(b.category) || a.name.localeCompare(b.name));

    return { version: metadata.version, presets, rows };
}

// ---------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------

/*
 * The manual's pages, with their hotkey directives rendered (see
 * hotkey-directives.mjs) and every `<video>` muted.
 */
function copyManual(dir, actions) {
    const manual = path.join(dir, 'manual');
    let count = 0;
    const walk = (from, to) => {
        fs.mkdirSync(to, { recursive: true });
        for (const entry of fs.readdirSync(from, { withFileTypes: true })) {
            const src = path.join(from, entry.name);
            const dest = path.join(to, entry.name);
            if (entry.isDirectory()) walk(src, dest);
            else if (entry.name.endsWith('.md')) {
                const where = path.relative(dir, src).split(path.sep).join('/');
                let md;
                try {
                    md = renderHotkeyDirectives(fs.readFileSync(src, 'utf8'), where, actions);
                } catch (e) {
                    fail(e.message);
                }
                fs.writeFileSync(dest, md.replace(/<video\b(?![^>]*\bmuted\b)/g, '<video muted'));
                count += 1;
            }
        }
    };
    walk(manual, OUT);
    return count;
}

function main() {
    const { dir, metadata, icons, assets } = source();
    const graphics = readGraphics(dir);

    fs.rmSync(OUT, { recursive: true, force: true });
    fs.mkdirSync(OUT, { recursive: true });

    const actions = actionsData(metadata, icons);
    fs.mkdirSync(DATA, { recursive: true });
    fs.writeFileSync(path.join(DATA, 'docs-actions.json'), `${JSON.stringify(actions)}\n`);
    fs.writeFileSync(path.join(DATA, 'hotkeys.css'), variantCss(actions.presets));

    const prose = copyManual(dir, actions);
    const media = copyPreviews(dir);
    const cards = copyGraphics(dir, graphics);

    let pages = 0;
    const missing = [];
    fs.mkdirSync(path.join(OUT, 'reference'), { recursive: true });
    for (const [section, ids] of Object.entries(SECTIONS)) {
        // One catalog is a page at the section's own path; several are a
        // directory under it. `docs-nav.ts` pins the matching shape.
        const flat = ids.length === 1;
        if (!flat) fs.mkdirSync(path.join(OUT, 'reference', section), { recursive: true });
        ids.forEach((id, i) => {
            const catalog = metadata.catalogs.find((c) => c.id === id);
            if (!catalog) {
                missing.push(id);
                return;
            }
            const slug = id.replace(/([a-z])([A-Z])/g, '$1-$2').toLowerCase();
            const file = flat
                ? path.join(OUT, 'reference', `${section}.md`)
                : path.join(OUT, 'reference', section, `${slug}.md`);
            const page = isGroupCatalog(catalog)
                ? groupPage(catalog, metadata, graphics, i + 1, metadata.version)
                : catalogPage(catalog, icons, assets, i + 1, metadata.version);
            fs.writeFileSync(file, page);
            pages += 1;
        });
    }

    // A renamed or dropped catalog is a real registry change, not something to
    // discover as a missing sidebar entry after deploy.
    if (missing.length) {
        fail(
            `metadata.json has no catalog(s): ${missing.join(', ')}\n` +
                `       Update SECTIONS in scripts/gen-docs.mjs to match the export.`,
        );
    }

    fs.writeFileSync(
        path.join(OUT, 'reference', 'actions.mdx'),
        [
            frontmatter({
                title: 'Actions & hotkeys',
                description: ACTIONS_INTRO,
                order: 1,
            }),
            "import ActionsTable from '../../../components/docs/ActionsTable.svelte';",
            '',
            ACTIONS_INTRO,
            '',
            '<ActionsTable client:load />',
            '',
        ].join('\n'),
    );

    log(
        `${metadata.version}: ${prose} prose page(s), ${pages} reference table(s), ` +
            `${actions.rows.length} actions across ${actions.presets.length} presets, ` +
            `${media} preview file(s), ${cards} card(s)`,
    );
}

main();
