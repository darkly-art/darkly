/**
 * Electron main process for the Darkly desktop bundle.
 *
 * - Sets up the platform-appropriate userData path (~/.config/Darkly/,
 *   etc.), with DARKLY_DATA_DIR as an override for portable installs.
 * - Creates a BrowserWindow that loads the frontend (a static build of
 *   frontend/), staged by `make app` into resources/app/.
 * - Wires up the storage IPC handlers that back window.electronAPI.storage
 *   on the renderer side.
 * - Chooses the Chromium GPU launch flags (gpu-launch.ts) and answers the
 *   renderer's WebGPU adapter report on window.electronAPI.gpu.
 */
import { app, BrowserWindow, Menu, ipcMain, dialog, shell } from 'electron';
import * as path from 'path';
import * as fs from 'fs';
import { createStorageHost } from './storage-host';
import { externalLinkHandler } from './external-links';
import {
    afterAdapterCheck,
    chooseProfile,
    launchDiagnostics,
    pinningEnv,
    readLaunchState,
    systemGpuCount,
    writeLaunchState,
    type GpuReport,
    type LaunchState,
} from './gpu-launch';

// On Windows, handle Squirrel install/uninstall events at startup. If we
// were launched as part of one, quit and let Squirrel do its thing.
// eslint-disable-next-line @typescript-eslint/no-require-imports
if (require('electron-squirrel-startup')) {
    app.quit();
}

// Use a clean "Darkly" name for the userData directory rather than the
// package.json "name" (which is "darkly-desktop"). This must happen before
// any app.getPath('userData') call.
app.setName('Darkly');

// The installed desktop entry's name, so Linux desktops pair the window with
// it: Electron derives the X11 WM_CLASS and the Wayland app_id from this, and
// the entry's StartupWMClass matches. The id is also APP_ID in the Makefile.
app.setDesktopName('art.darkly.Darkly.desktop');

// Allow overriding the userData path entirely for portable / sandboxed installs.
const dataDirOverride = process.env.DARKLY_DATA_DIR;
if (dataDirOverride) {
    app.setPath('userData', path.resolve(dataDirOverride));
}

// Keep every Vulkan user in the GPU process on the display GPU of a machine
// with more than one (see gpu-launch.ts). The variable only reaches the GPU
// process from the environment the app starts with, so set it and relaunch
// before anything else happens; app.exit ends this process on the spot, and
// the relaunched one finds the variable already set.
const gpuCount = process.platform === 'linux' ? systemGpuCount() : 0;
if (process.platform === 'linux') {
    const pin = pinningEnv(process.env, gpuCount);
    if (Object.keys(pin).length > 0) {
        console.log(`Darkly GPU launch: ${gpuCount} GPUs, relaunching with`, pin);
        Object.assign(process.env, pin);
        app.relaunch();
        app.exit(0);
    }
}

// Discard Chromium's on-disk GPU caches when the bundled Chromium version
// changes. A shader/GPU disk cache written by a previous Chromium major is not
// valid for the current one, and a stale cache leaves WebGPU with a working
// device but nothing presented: a fully blank canvas (the whole UI renders,
// the paint surface stays black). Chromium does not reliably invalidate these
// on upgrade, so we key a purge on process.versions.chrome and clear them
// before the GPU process starts reading them. Must run before app 'ready'.
function purgeGpuCaches(userData: string): void {
    // Every on-disk GPU/shader cache Chromium + Dawn write under userData.
    for (const dir of [
        'GPUCache',
        'DawnCache',
        'DawnGraphiteCache',
        'DawnWebGPUCache',
        'GrShaderCache',
        'ShaderCache',
    ]) {
        fs.rmSync(path.join(userData, dir), { recursive: true, force: true });
    }
}

// The launch state record: the Chromium version the caches belong to and, on
// Linux, the launch profile this session last worked with.
const launchStateFile = path.join(app.getPath('userData'), 'gpu-launch.json');
const storedLaunch = readLaunchState(launchStateFile);
const launch = process.platform === 'linux'
    ? chooseProfile(process.env, process.versions.chrome, storedLaunch)
    : null;

/** Record the launch state; true only if it reads back as written, so a
 *  relaunch can never repeat a profile because the record did not stick. */
function storeLaunchState(state: LaunchState): boolean {
    try {
        writeLaunchState(launchStateFile, state);
    } catch (err) {
        console.error('Darkly GPU launch: failed to record the launch state:', err);
        return false;
    }
    return JSON.stringify(readLaunchState(launchStateFile)) === JSON.stringify(state);
}

try {
    if (storedLaunch?.chrome !== process.versions.chrome) {
        purgeGpuCaches(app.getPath('userData'));
    }
    const state = launch?.state ?? { chrome: process.versions.chrome };
    if (JSON.stringify(state) !== JSON.stringify(storedLaunch)) {
        storeLaunchState(state);
    }
} catch (err) {
    console.error('Darkly: failed to purge stale GPU cache on upgrade:', err);
}

if (launch) {
    for (const [name, value] of launch.profile.switches) {
        app.commandLine.appendSwitch(name, value);
    }
    console.log(
        `Darkly GPU launch: profile ${launch.profile.id}${launch.pinned ? ' (DARKLY_GPU_PROFILE)' : ''}, ` +
            `session ${launch.state.session}, ${gpuCount} GPU(s)`,
    );
}

let mainWindow: BrowserWindow | null = null;

function createWindow() {
    mainWindow = new BrowserWindow({
        width: 1400,
        height: 900,
        minWidth: 800,
        minHeight: 600,
        title: 'Darkly',
        backgroundColor: '#1a1a1a',
        webPreferences: {
            preload: path.join(__dirname, 'preload.js'),
            nodeIntegration: false,
            contextIsolation: true,
            sandbox: true,
        },
    });

    // Locate the frontend resources.
    // - Packaged: process.resourcesPath/app/ (set via Forge's extraResource).
    // - electron-forge start: read from ../resources/app/ relative to dist/.
    const packagedResources = path.join(process.resourcesPath, 'app');
    const localResources = path.join(__dirname, '..', 'resources', 'app');
    const resourcesDir = fs.existsSync(packagedResources)
        ? packagedResources
        : localResources;

    const indexPath = path.join(resourcesDir, 'index.html');
    if (!fs.existsSync(indexPath)) {
        // Helpful error if the frontend wasn't built/staged.
        console.error(`Darkly: frontend not found at ${indexPath}`);
        console.error("Run 'make app' from the repo root to stage the frontend.");
    }
    // A link out of Darkly belongs in the reader's browser, not in a second
    // chromeless Electron window (Electron's default). See external-links.ts.
    mainWindow.webContents.setWindowOpenHandler(
        externalLinkHandler((url) => void shell.openExternal(url)),
    );

    mainWindow.loadFile(indexPath);

    if (process.env.DARKLY_DEVTOOLS) {
        mainWindow.webContents.openDevTools({ mode: 'detach' });
    }

    // The web frontend registers a `beforeunload` guard that cancels the unload
    // when a tab has unsaved changes (see App.svelte / closeGuard). In a browser
    // that produces a native "Leave site?" prompt, but Electron cancels the
    // unload silently, so the window's close button appears to do nothing.
    // Reproduce the browser prompt: when the renderer tries to block the close,
    // ask the user, and allow the unload (event.preventDefault) if they confirm.
    mainWindow.webContents.on('will-prevent-unload', (event) => {
        const choice = dialog.showMessageBoxSync(mainWindow!, {
            type: 'question',
            buttons: ['Quit', 'Cancel'],
            defaultId: 0,
            cancelId: 1,
            title: 'Unsaved changes',
            message: 'You have unsaved changes. Quit anyway?',
        });
        if (choice === 0) {
            event.preventDefault(); // allow the unload -> the window closes
        }
    });

    mainWindow.on('closed', () => {
        mainWindow = null;
    });
}

// Storage IPC handlers. The renderer's NodeFsStorage calls these via the
// preload bridge. All paths are validated inside storage-host.
const storageHost = createStorageHost(() => app.getPath('userData'));
ipcMain.handle('storage:read',   (_e, p: string)                  => storageHost.read(p));
ipcMain.handle('storage:write',  (_e, p: string, data: Uint8Array) => storageHost.write(p, data));
ipcMain.handle('storage:list',   (_e, dir: string)                => storageHost.list(dir));
ipcMain.handle('storage:remove', (_e, p: string)                  => storageHost.remove(p));
ipcMain.handle('storage:exists', (_e, p: string)                  => storageHost.exists(p));

// The renderer reports its WebGPU adapter check; the launch policy decides
// whether to move on to the next profile. Off Linux there is only one way to
// launch, so the answer is always to carry on.
ipcMain.handle('gpu:report', (_e, report: GpuReport) => {
    if (!launch) return { verdict: 'continue' };
    const outcome = afterAdapterCheck(process.env, launch, report.ok);
    console.log(
        `Darkly GPU launch: renderer ${report.ok ? `has ${report.adapter.vendor}/${report.adapter.architecture}` : report.reason}, ` +
            (outcome.verdict === 'relaunch' ? `relaunching on profile ${outcome.store.profile}` : outcome.verdict),
    );
    if (outcome.verdict === 'continue') return { verdict: 'continue' };
    if (storeLaunchState(outcome.store) && outcome.verdict === 'relaunch') {
        app.relaunch();
        app.exit(0);
        return { verdict: 'relaunching' };
    }
    return { verdict: 'exhausted', diagnostics: launchDiagnostics(process.env, launch, gpuCount, report) };
});

app.whenReady().then(() => {
    Menu.setApplicationMenu(null);
    createWindow();
});

app.on('window-all-closed', () => {
    // Standard macOS convention: leave the app running with no windows;
    // on other platforms, quit.
    if (process.platform !== 'darwin') {
        app.quit();
    }
});

app.on('activate', () => {
    // macOS: re-open a window when the dock icon is clicked.
    if (BrowserWindow.getAllWindows().length === 0) {
        createWindow();
    }
});
