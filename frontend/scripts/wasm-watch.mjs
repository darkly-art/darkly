// Rebuild the WASM bridge when Rust sources change, and reload the page.
//
// `npm run start` builds the bridge once (`wasm:build-dev`) and then hands off
// to Vite, which watches only `frontend/`. So every edit under `crates/` (a
// shader, a veil's `preview_at`, a registration) needed a manual `npm run
// wasm:build-dev` and a manual reload before it showed up in the editor,
// and a dev server left running silently served stale WASM. This closes
// that: the same watch-and-regenerate shape `iconBundlePlugin` uses for
// the icon bundle, applied to the thing the whole editor is compiled from.
//
// Dev only (`apply: 'serve'`). Production `npm run build` runs `wasm:build`
// ahead of Vite, so there is nothing to watch.

// @ts-nocheck: plain .mjs build tooling, outside the tsc src scope.
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const FRONTEND = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const WASM_CRATE = path.join(FRONTEND, 'wasm');
const PKG_ENTRY = path.join(WASM_CRATE, 'pkg', 'darkly_wasm.js');
const CRATES = path.join(FRONTEND, '..', 'crates');

// The bridge's whole input surface. `crates/` covers the engine, its shaders
// (`include_str!`), and its baked resources (`include_bytes!`); `wasm/` covers
// the bridge itself. Debounced coarsely rather than filtered finely; cargo
// decides what actually needs recompiling far better than a glob would.
const WATCH_ROOTS = [CRATES, path.join(WASM_CRATE, 'src'), path.join(WASM_CRATE, 'Cargo.toml')];

const SOURCE_RE = /\.(rs|wgsl|toml|yaml|yml|jpg|jpeg|png|webp)$/;

// `target/` is cargo's output and `pkg/` is wasm-pack's: watching either
// would make every build trigger the next one.
const IGNORED = [`${path.sep}target${path.sep}`, `${path.sep}pkg${path.sep}`];

// Coalesce the burst of events a single save (or a `cargo fmt`) produces.
const DEBOUNCE_MS = 150;

function isSource(file) {
    if (!SOURCE_RE.test(file)) return false;
    if (IGNORED.some((seg) => file.includes(seg))) return false;
    return WATCH_ROOTS.some((root) => file === root || file.startsWith(root + path.sep));
}

/// Run the dev bridge build, resolving `{ ok, output }`. Never rejects,
/// because a failed build is a normal state during editing, and the watcher
/// has to survive it to pick up the fix.
///
/// Goes through `npm run wasm:build-dev` rather than invoking `wasm-pack`
/// directly, for two reasons. The flags (`--dev --target web --out-dir pkg`)
/// are already defined once in `package.json` and should not be restated here.
/// And a bare `wasm-pack` spawned from Vite's process fails where the npm
/// script succeeds: `npm run` establishes the environment wasm-pack expects,
/// which spawning it out of a dev-server process does not.
function wasmPack() {
    return new Promise((resolve) => {
        const child = spawn(
            'npm',
            ['run', '--silent', 'wasm:build-dev'],
            { cwd: FRONTEND, stdio: ['ignore', 'pipe', 'pipe'] },
        );
        let output = '';
        child.stdout.on('data', (d) => { output += d; });
        child.stderr.on('data', (d) => { output += d; });
        child.on('error', (e) => resolve({ ok: false, output: `${e.message}\n${output}` }));
        child.on('close', (code) => resolve({ ok: code === 0, output }));
    });
}

export function wasmWatchPlugin() {
    let logger = console;
    let building = false;
    let queued = false;

    return {
        name: 'darkly-wasm-watch',
        apply: 'serve',
        configResolved(cfg) {
            logger = cfg.logger ?? console;
        },
        async configureServer(server) {
            // The crate lives outside Vite's root; watch it explicitly, the
            // same way the icon bundle plugin reaches `crates/darkly/src`.
            for (const root of WATCH_ROOTS) server.watcher.add(root);

            const rebuild = async (reason) => {
                // One build at a time; a change that lands mid-build queues
                // exactly one more rather than piling up a build per keystroke.
                if (building) {
                    queued = true;
                    return;
                }
                building = true;
                logger.info(`[wasm] ${reason}: rebuilding…`);
                const started = Date.now();
                const { ok, output } = await wasmPack();
                building = false;

                if (ok) {
                    const secs = ((Date.now() - started) / 1000).toFixed(1);
                    logger.info(`[wasm] rebuilt in ${secs}s: reloading`);
                    server.ws.send({ type: 'full-reload', path: '*' });
                } else {
                    logger.error(`[wasm] build failed\n${output}`);
                    server.ws.send({
                        type: 'error',
                        err: {
                            message: 'wasm build failed: see the terminal',
                            stack: output,
                            plugin: 'darkly-wasm-watch',
                        },
                    });
                }

                if (queued) {
                    queued = false;
                    await rebuild('changed during the last build');
                }
            };

            // `npm run start` builds before Vite starts, but `npm run dev` on a
            // fresh checkout does not; without this the bridge import fails to
            // resolve with nothing pointing at why.
            if (!fs.existsSync(PKG_ENTRY)) {
                await rebuild('no bridge built yet');
            }

            let timer = null;
            const onChange = (file) => {
                if (!isSource(file)) return;
                clearTimeout(timer);
                timer = setTimeout(
                    () => rebuild(`${path.relative(CRATES, file)} changed`),
                    DEBOUNCE_MS,
                );
            };
            server.watcher.on('change', onChange);
            server.watcher.on('add', onChange);
            server.watcher.on('unlink', onChange);
        },
    };
}
