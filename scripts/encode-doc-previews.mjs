#!/usr/bin/env node
/*
 * Turn `render-docs`' PNG frame sequences into what a documentation page can
 * actually embed: one poster still plus one looping H.264 video per entry.
 *
 *   cargo run -p darkly --features testing --bin render_docs -- --out frames
 *   node scripts/encode-doc-previews.mjs --frames frames --out out
 *
 * Raw frames were always the intermediate, never the deliverable: the last
 * full render is 1 599 PNGs and 75 MB, which is not something to put on a
 * release, let alone in a table. H.264 over the same frames is a few MB.
 *
 * This lives here rather than in `crates/darkly` on purpose: encoding is one
 * ffmpeg invocation per entry, and folding it into `render-docs` would add a
 * codec dependency to the editor's own crate to save a shell loop. The
 * documentation workflow runs it the same way it runs the icon resolver, so the
 * consumer never sees a frame.
 *
 * ## What it writes
 *
 *   <out>/assets.json                   the index: same shape as the frame
 *                                       index, with `poster`/`video` where it
 *                                       had `dir`/`frames`/`still`
 *   <out>/previews/<catalog>/<id>.webp  the frame `render-docs` nominated
 *   <out>/previews/<catalog>/<id>.mp4   the loop, absent for a still-only entry
 *
 * A single-frame entry (every brush is one, they declare `PreviewAnim::STILL`)
 * gets a poster and no video. `video` being absent is how a consumer knows
 * there is nothing to play, so it can render a plain image rather than a
 * `<video>` that will never move.
 *
 * ## Determinism
 *
 * Encoding is bit-exact (`-fflags +bitexact`), so the same frames encode to the
 * same bytes and a rebuild of an unchanged registry does not churn the release
 * asset. That is the same property the preview stroke seed was made explicit
 * for; it is worth as much here, one layer further out.
 */

import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

/*
 * Constant quality rather than a bitrate: the catalogs differ enormously in
 * how hard they are to encode (a blend-mode sweep is two smooth fields
 * crossfading, the noise void is full-frame grain), and one bitrate would
 * either starve the grain or waste bytes on the gradients. 20 is a little
 * finer than the 23 default because these are 256 px squares read at a
 * glance for exactly the difference between one effect and the next.
 */
const CRF = '20';

/** Encoding runs once per release, so the slow preset costs nothing that matters. */
const PRESET = 'veryslow';

/*
 * Posters are lossy WebP, not the PNG the renderer wrote.
 *
 * They are half the frames' pixels but they were 51 % of the encoded artifact:
 * a 256 px square of smooth gradient costs 70 KB as PNG and 3 KB as WebP, and
 * the two are indistinguishable at the size a table cell shows them. Every
 * poster on a page loads eagerly (that is what makes the video's `preload:
 * none` affordable), so this is the number that decides what a reader waits
 * for: 1.9 MB of posters across 48 entries becomes 0.1 MB.
 *
 * These previews are gradients, fields and strokes: no text, no hard edges,
 * nothing that shows ringing. The lossless frames are still the intermediate;
 * nothing downstream reads a poster as reference pixels.
 */
const POSTER_QUALITY = '88';

const log = (msg) => console.log(`[previews] ${msg}`);

function fail(msg) {
    console.error(`\n[previews] ${msg}\n`);
    process.exit(1);
}

function parseArgs(argv) {
    const args = { frames: null, out: null };
    for (let i = 0; i < argv.length; i += 1) {
        const flag = argv[i];
        if (flag === '--frames') args.frames = argv[++i];
        else if (flag === '--out') args.out = argv[++i];
        else if (flag === '--help') args.help = true;
        else fail(`unknown argument: ${flag}`);
    }
    return args;
}

const USAGE = `\
encode-doc-previews - poster + looping H.264 per previewable entry

USAGE:
    node scripts/encode-doc-previews.mjs --frames <dir> --out <dir>

OPTIONS:
    --frames <dir>  A render_docs output directory: assets.json plus one
                    directory of numbered PNGs per entry
    --out <dir>     Artifact directory to write assets.json and previews/ into
`;

function ffmpeg(args) {
    const res = spawnSync('ffmpeg', args, { encoding: 'utf8' });
    if (res.error) fail(`could not run ffmpeg: ${res.error.message}`);
    if (res.status !== 0) {
        fail(`ffmpeg failed (${res.status}):\n${res.stderr?.trim() ?? ''}`);
    }
}

/**
 * One entry's loop.
 *
 * `yuv420p` because it is the only pixel format that plays everywhere, and it
 * is why the even-dimension check below is fatal rather than a silent rescale:
 * chroma subsampling cannot represent an odd edge, and quietly resizing a
 * documentation asset to a size its manifest does not claim is worse than
 * stopping.
 */
function encode(dir, asset, dest) {
    ffmpeg([
        '-nostdin',
        '-y',
        '-loglevel', 'error',
        // Bit-exact: no encoder version string in the container, no wall-clock
        // creation time, so identical frames give an identical file.
        '-fflags', '+bitexact',
        '-flags', '+bitexact',
        '-framerate', String(asset.fps),
        '-i', path.join(dir, '%03d.png'),
        '-c:v', 'libx264',
        '-preset', PRESET,
        '-crf', CRF,
        '-pix_fmt', 'yuv420p',
        '-an',
        // The player restarts the file rather than seeking, so the only
        // keyframe that has to exist is the first one.
        '-movflags', '+faststart',
        dest,
    ]);
}

function main() {
    const args = parseArgs(process.argv.slice(2));
    if (args.help || !args.frames || !args.out) {
        process.stdout.write(USAGE);
        process.exit(args.help ? 0 : 1);
    }

    const frames = path.resolve(args.frames);
    const out = path.resolve(args.out);
    const index = path.join(frames, 'assets.json');
    if (!fs.existsSync(index)) {
        fail(`${index} does not exist; run render_docs --out ${args.frames} first`);
    }
    const manifest = JSON.parse(fs.readFileSync(index, 'utf8'));

    const media = path.join(out, 'previews');
    fs.rmSync(media, { recursive: true, force: true });

    const assets = {};
    let videos = 0;
    let stills = 0;

    for (const [catalog, entries] of Object.entries(manifest.assets)) {
        fs.mkdirSync(path.join(media, catalog), { recursive: true });
        assets[catalog] = {};

        for (const [id, asset] of Object.entries(entries)) {
            const dir = path.join(frames, asset.dir);
            if (!fs.existsSync(dir)) fail(`assets.json names ${asset.dir}, which does not exist`);
            if (asset.width % 2 || asset.height % 2) {
                fail(`${catalog}/${id} is ${asset.width} × ${asset.height}: H.264 needs even edges`);
            }

            const poster = path.join('previews', catalog, `${id}.webp`);
            ffmpeg([
                '-nostdin',
                '-y',
                '-loglevel', 'error',
                '-fflags', '+bitexact',
                '-flags', '+bitexact',
                '-i', path.join(dir, `${String(asset.still).padStart(3, '0')}.png`),
                '-c:v', 'libwebp',
                '-quality', POSTER_QUALITY,
                path.join(out, poster),
            ]);

            const entry = {
                poster,
                width: asset.width,
                height: asset.height,
                loop: asset.loop,
            };

            // One frame is a still by declaration, not a one-frame film.
            if (asset.frames > 1) {
                const video = path.join('previews', catalog, `${id}.mp4`);
                encode(dir, asset, path.join(out, video));
                entry.video = video;
                entry.frames = asset.frames;
                entry.fps = asset.fps;
                videos += 1;
            } else {
                stills += 1;
            }

            assets[catalog][id] = entry;
        }
    }

    fs.mkdirSync(out, { recursive: true });
    fs.writeFileSync(
        path.join(out, 'assets.json'),
        `${JSON.stringify({ version: manifest.version, assets }, null, 2)}\n`,
    );

    const bytes = (dir) =>
        fs
            .readdirSync(dir, { recursive: true, withFileTypes: true })
            .filter((e) => e.isFile())
            .reduce((sum, e) => sum + fs.statSync(path.join(e.parentPath, e.name)).size, 0);

    log(
        `${manifest.version}: ${videos} loop(s), ${stills} still(s), ` +
            `${(bytes(media) / 1024 / 1024).toFixed(1)} MB`,
    );
}

main();
