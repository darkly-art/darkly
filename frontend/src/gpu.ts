import type { ToastLevel } from './state/toast.svelte';

interface GpuCheckResult {
    level: ToastLevel;
    message: string;
}

/** Known software renderer identifiers (case-insensitive substring match). */
const SOFTWARE_RENDERERS = [
    'swiftshader',
    'softpipe',
    'llvmpipe',
    'software rasterizer',
    'microsoft basic render',
];

function isSoftwareRenderer(description: string): boolean {
    const lower = description.toLowerCase();
    return SOFTWARE_RENDERERS.some(name => lower.includes(name));
}

/**
 * Probes the WebGPU adapter to determine whether hardware acceleration
 * is active. Returns a toast-ready result.
 */
export async function checkGpu(): Promise<GpuCheckResult> {
    if (!navigator.gpu) {
        return {
            level: 'error',
            message: 'WebGPU is not supported in this browser.',
        };
    }

    const adapter = await navigator.gpu.requestAdapter({
        powerPreference: 'high-performance',
    });

    if (!adapter) {
        return {
            level: 'error',
            message: 'No GPU adapter found. Hardware acceleration may be disabled.',
        };
    }

    const info = adapter.info;
    const description = info.description || info.device || '';
    const vendor = info.vendor || '';
    const label = description || vendor || 'Unknown GPU';

    if (info.isFallbackAdapter || isSoftwareRenderer(description) || isSoftwareRenderer(vendor)) {
        return {
            level: 'warning',
            message: `Software renderer detected (${label}). Enable hardware acceleration for best performance.`,
        };
    }

    return {
        level: 'success',
        message: `GPU: ${label}`,
    };
}
