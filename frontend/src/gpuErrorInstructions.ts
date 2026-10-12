// Last reviewed: 2026-10
//
// Per (os, browser) instructions for enabling hardware-accelerated WebGPU.
// Keep URLs in code blocks, not anchors: chrome://... cannot be opened from
// a webpage and rendering them as <a href> looks broken.

import type { Browser, Os, Platform } from './platform';

export interface FlagLink {
    /** Displayed inline as code with a copy-to-clipboard button. */
    url: string;
    /** What the artist should change once they reach the URL. */
    action: string;
}

export interface Instructions {
    /** Short human-readable label, e.g. "Chromium on Linux". */
    title: string;
    /** Ordered steps for the artist to follow. */
    steps: string[];
    /** URLs to surface with copy buttons (flags, settings pages). */
    flags: FlagLink[];
    /** Diagnostic URL (e.g. chrome://gpu). Optional. */
    diagnosticUrl?: string;
    /** Extra context, caveats, or "many devices simply won't work" notes. */
    note?: string;
}

const CHROMIUM_DESKTOP_STEPS: string[] = [
    'Open your browser\'s system settings page (see below).',
    'Turn on "Use graphics acceleration when available".',
    'Restart the browser.',
    'Reload this page.',
];

const CHROMIUM_DESKTOP_FLAGS: FlagLink[] = [
    { url: 'chrome://settings/system', action: 'Chrome' },
    { url: 'edge://settings/system', action: 'Edge' },
    { url: 'brave://settings/system', action: 'Brave' },
    { url: 'opera://settings/system', action: 'Opera' },
];

/** A hybrid laptop's black canvas: Vulkan on the discrete GPU while the
 *  display runs on the integrated one. Mesa's device-select layer settles it. */
const HYBRID_NOTE =
    'On a laptop with two GPUs, a black canvas usually means Vulkan picked the GPU that does not drive the display. '
    + 'Launching with MESA_VK_DEVICE_SELECT_FORCE_DEFAULT_DEVICE=1 keeps every Vulkan user on the display GPU; '
    + 'MESA_VK_DEVICE_SELECT=vid:did! picks one yourself (MESA_VK_DEVICE_SELECT=list vulkaninfo prints the ids).';

const INSTRUCTIONS_MATRIX: Partial<Record<Browser, Partial<Record<Os, Instructions>>>> = {
    // The Darkly desktop app: no browser settings to change, and on Linux the
    // app has already tried every Chromium launch profile it knows
    // (desktop/src/gpu-launch.ts) before showing this.
    electron: {
        windows: {
            title: 'Darkly desktop on Windows',
            steps: [
                'Update your graphics drivers from your GPU vendor\'s site (NVIDIA, AMD, or Intel).',
                'Restart Darkly.',
            ],
            flags: [],
        },
        macos: {
            title: 'Darkly desktop on macOS',
            steps: [
                'Update macOS: System Settings → General → Software Update.',
                'Restart Darkly.',
            ],
            flags: [],
        },
        linux: {
            title: 'Darkly desktop on Linux',
            steps: [
                'Darkly tried Chromium\'s default GPU path and its Vulkan backend, and neither gave it a GPU. The details below say what it saw.',
                'Update your graphics drivers: Mesa for Intel and AMD, driver 535 or newer for NVIDIA. The first command below should list your GPU.',
                'If you launch Darkly with GPU flags of your own (such as --disable-features=Vulkan or --disable-gpu), remove them.',
                'Restart Darkly.',
            ],
            flags: [
                { url: 'vulkaninfo --summary', action: 'lists the GPUs Vulkan can use' },
                { url: 'MESA_VK_DEVICE_SELECT=list vulkaninfo', action: 'prints their vid:did ids' },
            ],
            note: `${HYBRID_NOTE} Darkly sets the first on its own when it finds two GPUs. `
                + 'VK_LOADER_DRIVERS_DISABLE=*nvidia* hides a driver, NODEVICE_SELECT=1 turns device selection off, '
                + 'and DARKLY_GPU_PROFILE=defaults or vulkan forces a Chromium launch profile. '
                + 'For the Flatpak, set any of these with flatpak override --user --env=NAME=value art.darkly.Darkly.',
        },
    },
    chromium: {
        windows: {
            title: 'Chromium on Windows',
            steps: [
                ...CHROMIUM_DESKTOP_STEPS,
                'If the error persists, update your graphics drivers from your GPU vendor\'s site (NVIDIA, AMD, or Intel).',
            ],
            flags: CHROMIUM_DESKTOP_FLAGS,
            diagnosticUrl: 'chrome://gpu',
        },
        macos: {
            title: 'Chromium on macOS',
            steps: [
                ...CHROMIUM_DESKTOP_STEPS,
                'If the error persists, update macOS via System Settings → General → Software Update.',
            ],
            flags: CHROMIUM_DESKTOP_FLAGS,
            diagnosticUrl: 'chrome://gpu',
        },
        linux: {
            title: 'Chromium on Linux',
            steps: [
                ...CHROMIUM_DESKTOP_STEPS,
                'On Wayland with an Intel GPU from 12th gen on (Mesa 22 or newer) or an NVIDIA GPU (driver 535 or newer), that is all WebGPU needs.',
                'On X11, or with an AMD or older Intel GPU, Chromium also needs its Vulkan backend: set the Vulkan flag below to "Enabled" (or launch with --enable-features=Vulkan) and relaunch.',
                'If the error persists, set "Override software rendering list" below to "Enabled" and relaunch.',
            ],
            flags: [
                ...CHROMIUM_DESKTOP_FLAGS,
                { url: 'chrome://flags/#enable-vulkan', action: 'Enabled' },
                { url: 'chrome://flags/#ignore-gpu-blocklist', action: 'Enabled (last resort)' },
            ],
            diagnosticUrl: 'chrome://gpu',
            note: `Make sure your Mesa/Vulkan drivers are up to date: Vulkan 1.1+ is required. ${HYBRID_NOTE}`,
        },
        chromeos: {
            title: 'Chromium on ChromeOS',
            steps: [
                ...CHROMIUM_DESKTOP_STEPS,
                'If the error persists, enable the flags below and relaunch.',
            ],
            flags: [
                { url: 'chrome://settings/system', action: 'Chrome' },
                { url: 'chrome://flags/#enable-unsafe-webgpu', action: 'Enabled' },
            ],
            diagnosticUrl: 'chrome://gpu',
        },
        android: {
            title: 'Chrome on Android',
            steps: [
                'Open chrome://flags in a new tab.',
                'Find "Unsafe WebGPU" and set it to Enabled.',
                'Relaunch Chrome when prompted.',
                'Reload this page.',
            ],
            flags: [{ url: 'chrome://flags/#enable-unsafe-webgpu', action: 'Enabled' }],
            note: 'Many Android devices do not yet have a WebGPU-capable driver. If the flag is already enabled and it still fails, your device may not be supported.',
        },
    },
    firefox: {
        windows: firefoxDesktop(),
        macos: firefoxDesktop(),
        linux: {
            ...firefoxDesktop(),
            note: 'On Linux, WebGPU additionally requires Vulkan 1.1+. Check your Mesa / GPU drivers if the flag is enabled but the error persists.',
        },
    },
    safari: {
        macos: {
            title: 'Safari on macOS',
            steps: [
                'Safari 26 and newer ship with WebGPU enabled by default, so this error usually means something else is wrong.',
                'Update macOS: System Settings → General → Software Update.',
                'On older Safari versions: enable Develop → Feature Flags → WebGPU (enable the Develop menu first in Settings → Advanced).',
                'Check Activity Monitor → GPU process to confirm your GPU isn\'t disabled.',
            ],
            flags: [],
        },
        ios: {
            title: 'Safari on iOS / iPadOS',
            steps: [
                'Open the Settings app.',
                'Scroll to Safari → Advanced → Feature Flags.',
                'Enable "WebGPU".',
                'Return here and reload.',
            ],
            flags: [],
            note: 'All iOS browsers (Chrome, Firefox, Edge on iPhone/iPad) use WebKit: the setting above applies regardless of which browser you\'re using. Older iOS devices may not support WebGPU at all.',
        },
    },
};

function firefoxDesktop(): Instructions {
    return {
        title: 'Firefox',
        steps: [
            'Open about:config in a new tab and accept the warning.',
            'Search for dom.webgpu.enabled and set it to true.',
            'Also verify Settings → General → Performance → "Use hardware acceleration when available" is on.',
            'Restart Firefox and reload this page.',
        ],
        flags: [
            { url: 'about:config', action: 'Set dom.webgpu.enabled = true' },
        ],
    };
}

const FALLBACK_INSTRUCTIONS: Instructions = {
    title: 'Generic instructions',
    steps: [
        'We could not identify your browser precisely.',
        'Look for a "Hardware acceleration" toggle in your browser\'s settings and enable it.',
        'Check your browser\'s documentation for "enable WebGPU"; it may be behind a feature flag.',
        'Make sure your graphics drivers are up to date.',
        'Restart the browser and reload this page.',
    ],
    flags: [],
};

export function instructionsFor(platform: Platform): Instructions {
    return INSTRUCTIONS_MATRIX[platform.browser]?.[platform.os] ?? FALLBACK_INSTRUCTIONS;
}

/**
 * Returns all instruction sets we know about, for the "other platforms"
 * collapsible section. Keyed by a stable identifier.
 */
export function allInstructions(): Array<{ key: string; instructions: Instructions }> {
    const out: Array<{ key: string; instructions: Instructions }> = [];
    const browsers = Object.keys(INSTRUCTIONS_MATRIX) as Browser[];
    for (const browser of browsers) {
        const byOs = INSTRUCTIONS_MATRIX[browser];
        if (!byOs) continue;
        const oses = Object.keys(byOs) as Os[];
        for (const os of oses) {
            const instr = byOs[os];
            if (instr) out.push({ key: `${browser}-${os}`, instructions: instr });
        }
    }
    return out;
}
