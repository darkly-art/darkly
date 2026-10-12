/**
 * Desktop smoke test. Runs against the unpacked output of
 * `electron-forge package` (which `electron-forge make` also produces): the
 * desktop/out/Darkly-<platform>-<arch>/ directory. With DARKLY_EXECUTABLE set
 * it runs that binary instead: CI points it at the `darkly` symlink
 * `make install` stages, so the installed tree is what boots.
 *
 * Scope: verify that the desktop wrapper boots, a window opens with the
 * expected title, the preload bridge exposes window.electronAPI.storage, and
 * on Linux the GPU launch profile's switches reach Chromium.
 * NOT a feature test: does not touch WebGPU, canvas rendering, drawing, or
 * file I/O. Feature behavior (including canvas mount) lives in the public
 * repo's Vitest suite, where a real GPU/Vulkan stack is available.
 *
 * Why clear ELECTRON_RUN_AS_NODE in the launch env: VS Code's extension
 * host sets it to 1 so child Electron processes act as plain Node. That
 * env var leaks into any terminal launched from VS Code, and an
 * unrelated test environment may inherit it too. Forcing it empty is
 * the only reliable way to ensure the packaged binary boots as Electron.
 */
import { _electron as electron, type ElectronApplication } from 'playwright';
import { test, expect } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';

/** Resolve the packaged binary for the current host platform. */
function packagedBinary(): string {
    const outDir = path.join(__dirname, '..', 'out');
    const candidates = fs.readdirSync(outDir).filter(d => d.startsWith('Darkly-'));
    if (candidates.length !== 1) {
        throw new Error(
            `expected exactly one packaged Darkly-* directory under ${outDir}, ` +
            `found ${candidates.length}: ${JSON.stringify(candidates)}. ` +
            `Run 'make app' and then 'npx electron-forge package' first.`,
        );
    }
    const base = path.join(outDir, candidates[0]);
    if (process.platform === 'darwin') {
        return path.join(base, 'Darkly.app', 'Contents', 'MacOS', 'Darkly');
    }
    if (process.platform === 'win32') {
        return path.join(base, 'darkly.exe');
    }
    return path.join(base, 'darkly');
}

/** Env for the child Electron process, explicitly free of any leakage
 *  that would make it act as Node instead of Electron. The GPU launch
 *  profile and device pinning are fixed up front: either would otherwise
 *  relaunch the app out from under Playwright on some machines (see
 *  src/gpu-launch.ts). */
function launchEnv(): NodeJS.ProcessEnv {
    const env = { ...process.env };
    delete env.ELECTRON_RUN_AS_NODE;
    env.DARKLY_GPU_PROFILE = 'vulkan';
    env.MESA_VK_DEVICE_SELECT_FORCE_DEFAULT_DEVICE ??= '1';
    return env;
}

test('packaged app launches, renders, and exposes the storage bridge', async () => {
    let app: ElectronApplication | undefined;
    try {
        app = await electron.launch({
            executablePath: process.env.DARKLY_EXECUTABLE || packagedBinary(),
            // --no-sandbox: the packaged binary's chrome-sandbox is not setuid
            //   root when run from arbitrary user paths (CI runners, /tmp, etc.).
            //   Disabling Chromium's setuid sandbox lets Playwright spawn the
            //   renderer reliably without a privileged install step. End users
            //   running the AppImage from a normal install get the standard
            //   sandboxed behavior.
            args: process.platform === 'linux' ? ['--no-sandbox'] : [],
            env: launchEnv(),
            timeout: 60_000,
        });

        const win = await app.firstWindow({ timeout: 30_000 });

        // The window title is the frontend's <title>: the app's name alone,
        // with no tagline.
        await expect(win).toHaveTitle('Darkly', { timeout: 30_000 });

        // Intentionally no canvas/render check here. CI runners on Linux
        // (xvfb, no GPU) and the Intel-mac runner have no working WebGPU
        // backend, so the Svelte app's canvas may never mount even when the
        // bundle is fine. Canvas/WebGPU behavior is covered in the public
        // repo's Vitest suite; this test only validates that the desktop
        // wrapper boots and exposes its preload contract.

        // Preload must have injected the storage bridge. This is the contract
        // the public repo's NodeFsStorage adapter depends on at runtime.
        const hasBridge = await win.evaluate(() =>
            typeof (window as Window & { electronAPI?: { storage?: { read?: unknown } } })
                .electronAPI?.storage?.read === 'function'
        );
        expect(hasBridge).toBe(true);

        // The Linux launch profile reaches Chromium's command line, and the
        // unsafe-webgpu switch, which only ever handed out a software adapter
        // where there was no hardware one, is gone.
        if (process.platform === 'linux') {
            const switches = await app.evaluate(({ app }) => ({
                unsafe: app.commandLine.hasSwitch('enable-unsafe-webgpu'),
                features: app.commandLine.getSwitchValue('enable-features'),
            }));
            expect(switches.unsafe).toBe(false);
            expect(switches.features).toContain('Vulkan');
        }
    } finally {
        await app?.close();
    }
});
