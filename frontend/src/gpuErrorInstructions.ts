// Last reviewed: 2026-04
//
// Per (os, browser) instructions for enabling hardware-accelerated WebGPU.
// Keep URLs in code blocks, not anchors — chrome://... cannot be opened from
// a webpage and rendering them as <a href> looks broken.

import type { Browser, Os, Platform } from './platform';

export interface FlagLink {
    /** Displayed inline as code with a copy-to-clipboard button. */
    url: string;
    /** What the user should change once they reach the URL. */
    action: string;
}

export interface Instructions {
    /** Short human-readable label, e.g. "Chromium on Linux". */
    title: string;
    /** Ordered steps for the user to follow. */
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

const INSTRUCTIONS_MATRIX: Partial<Record<Browser, Partial<Record<Os, Instructions>>>> = {
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
                'Linux GPUs are often on Chromium\'s blocklist. If the error persists, set the flags below to "Enabled" and relaunch.',
            ],
            flags: [
                ...CHROMIUM_DESKTOP_FLAGS,
                { url: 'chrome://flags/#ignore-gpu-blocklist', action: 'Enabled' },
                { url: 'chrome://flags/#enable-unsafe-webgpu', action: 'Enabled (last resort)' },
            ],
            diagnosticUrl: 'chrome://gpu',
            note: 'Make sure your Mesa/Vulkan drivers are up to date. Vulkan 1.1+ is required for WebGPU on Linux.',
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
            note: 'All iOS browsers (Chrome, Firefox, Edge on iPhone/iPad) use WebKit — the setting above applies regardless of which browser you\'re using. Older iOS devices may not support WebGPU at all.',
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
        'Check your browser\'s documentation for "enable WebGPU" — it may be behind a feature flag.',
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
