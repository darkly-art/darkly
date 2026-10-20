// Drives `stroke-replay.html` through a headless Chromium over the DevTools
// protocol: visits each URL in turn, waits for the page to set its title to
// `done`, and prints `window.__result` as one JSON line per URL. Node 22
// built-ins only (`fetch`, `WebSocket`).
//
//   HEADLESS=1 node scripts/bench-drive.mjs 'https://localhost:5173/stroke-replay.html?brush=Pencil&stab=0.6&radius=500&pace=sync'
//
// `CHROME` names the binary (default `chromium`), `PROFILE` the user-data
// directory (default under the OS temp dir), `HEADLESS` runs without a
// window, which is also what keeps the frame clock running when no window
// system is watching. The dev server is https with a self-signed
// certificate, hence `--ignore-certificate-errors`.
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const urls = process.argv.slice(2);
if (urls.length === 0) {
    console.error('usage: bench-drive.mjs <url>...');
    process.exit(2);
}
const port = 9333;
const profile = process.env.PROFILE ?? path.join(os.tmpdir(), 'darkly-bench-profile');
fs.mkdirSync(profile, { recursive: true });
const chrome = spawn(
    process.env.CHROME ?? 'chromium',
    [
        `--remote-debugging-port=${port}`,
        `--user-data-dir=${profile}`,
        '--ignore-certificate-errors',
        '--enable-unsafe-webgpu',
        '--no-first-run',
        '--no-default-browser-check',
        '--window-size=1960,1200',
        '--disable-background-timer-throttling',
        '--disable-renderer-backgrounding',
        ...(process.env.HEADLESS ? ['--headless=new', '--use-angle=vulkan', '--enable-features=Vulkan'] : []),
        'about:blank',
    ],
    { stdio: 'ignore' },
);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function targets() {
    try {
        return await (await fetch(`http://localhost:${port}/json`)).json();
    } catch {
        return null;
    }
}
let list = null;
for (let i = 0; i < 100 && !list; i++) {
    list = await targets();
    if (!list) await sleep(200);
}
if (!list) {
    chrome.kill('SIGTERM');
    throw new Error('chromium did not open its debugging port');
}
const page = list.find((t) => t.type === 'page');
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0;
const pending = new Map();
const consoleLines = [];
ws.onmessage = (m) => {
    const d = JSON.parse(m.data);
    if (d.id && pending.has(d.id)) {
        pending.get(d.id)(d);
        pending.delete(d.id);
    } else if (d.method === 'Runtime.exceptionThrown') {
        const x = d.params.exceptionDetails;
        consoleLines.push(`exception: ${(x.exception?.description ?? x.text).slice(0, 300)}`);
    }
};
const call = (method, params = {}) =>
    new Promise((r) => {
        const i = ++id;
        pending.set(i, r);
        ws.send(JSON.stringify({ id: i, method, params }));
    });
const evalJs = async (expression) =>
    (await call('Runtime.evaluate', { expression, returnByValue: true })).result?.result?.value;
await call('Page.enable');
await call('Runtime.enable');
for (const url of urls) {
    await call('Page.navigate', { url });
    const start = Date.now();
    let res;
    while (Date.now() - start < 120000) {
        await sleep(500);
        if ((await evalJs('document.title')) === 'done') {
            res = await evalJs('JSON.stringify(window.__result)');
            break;
        }
    }
    console.log(JSON.stringify({ url, result: res ? JSON.parse(res) : 'timeout', console: consoleLines.splice(0) }));
}
ws.close();
chrome.kill('SIGTERM');
