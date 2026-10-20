import type { ElectronGpuBridge } from './host';

export interface AdapterInfo {
    vendor: string;
    architecture: string;
    device: string;
    description: string;
}

export type GpuCheckFailure =
    | { reason: 'no-webgpu' }
    | { reason: 'no-adapter' }
    | { reason: 'fallback-adapter'; adapterInfo: AdapterInfo };

export type GpuCheckResult =
    | { ok: true; adapterInfo: AdapterInfo }
    | ({ ok: false } & GpuCheckFailure);

const ADAPTER_RETRY_DELAY_MS = 150;

async function requestAdapterWithRetry(): Promise<GPUAdapter | null> {
    const first = await navigator.gpu.requestAdapter({
        powerPreference: 'high-performance',
    });
    if (first) return first;

    await new Promise(resolve => setTimeout(resolve, ADAPTER_RETRY_DELAY_MS));
    return navigator.gpu.requestAdapter({
        powerPreference: 'high-performance',
    });
}

function readAdapterInfo(adapter: GPUAdapter): AdapterInfo {
    const info = adapter.info;
    return {
        vendor: info.vendor ?? '',
        architecture: info.architecture ?? '',
        device: info.device ?? '',
        description: info.description ?? '',
    };
}

/**
 * Strict WebGPU availability check. Only fails on reliable signals:
 * missing `navigator.gpu`, null adapter after one retry, or
 * `adapter.info.isFallbackAdapter === true` (the W3C-spec signal that the
 * browser itself fell back to software).
 *
 * Real-GPU artists must never see a failure here.
 */
export async function checkGpu(): Promise<GpuCheckResult> {
    if (!navigator.gpu) {
        return { ok: false, reason: 'no-webgpu' };
    }

    const adapter = await requestAdapterWithRetry();
    if (!adapter) {
        return { ok: false, reason: 'no-adapter' };
    }

    const adapterInfo = readAdapterInfo(adapter);

    if (adapter.info.isFallbackAdapter) {
        return { ok: false, reason: 'fallback-adapter', adapterInfo };
    }

    return { ok: true, adapterInfo };
}

/** What the renderer tells the desktop host about its adapter check. */
export type GpuReport =
    | { ok: false; reason: GpuCheckFailure['reason'] }
    | { ok: true; adapter: AdapterInfo };

/** The desktop host's answer. `relaunching`: the host is restarting the app
 *  with other Chromium flags, so nothing should mount. `exhausted`: no launch
 *  profile produced an adapter; `diagnostics` belong on the error page. */
export type HostVerdict =
    | { verdict: 'continue' }
    | { verdict: 'relaunching' }
    | { verdict: 'exhausted'; diagnostics: string[] };

/** Report a check to the desktop host and return its verdict. A host that
 *  fails to answer must not keep the app from booting, so that reads as
 *  `continue`. */
export async function reportToHost(check: GpuCheckResult, bridge: ElectronGpuBridge): Promise<HostVerdict> {
    const report: GpuReport = check.ok
        ? { ok: true, adapter: check.adapterInfo }
        : { ok: false, reason: check.reason };
    try {
        return await bridge.report(report);
    } catch (err) {
        console.error('Darkly: the desktop host did not answer the GPU report:', err);
        return { verdict: 'continue' };
    }
}
