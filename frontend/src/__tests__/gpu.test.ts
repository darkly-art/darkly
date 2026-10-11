import { describe, it, expect, vi } from 'vitest';
import { reportToHost, type GpuReport, type HostVerdict } from '../gpu';
import type { ElectronGpuBridge } from '../host';

const ADAPTER = { vendor: 'intel', architecture: 'gen-12lp', device: '', description: '' };

/** A bridge that records what it was told and answers with `verdict`. */
function fakeBridge(verdict: HostVerdict): ElectronGpuBridge & { reports: GpuReport[] } {
    const reports: GpuReport[] = [];
    return {
        reports,
        async report(r) {
            reports.push(r);
            return verdict;
        },
    };
}

describe('reportToHost', () => {
    it('reports a failed check by its reason', async () => {
        const bridge = fakeBridge({ verdict: 'relaunching' });
        await reportToHost({ ok: false, reason: 'no-adapter' }, bridge);
        expect(bridge.reports).toEqual([{ ok: false, reason: 'no-adapter' }]);
    });

    it('reports a passing check with its adapter', async () => {
        const bridge = fakeBridge({ verdict: 'continue' });
        await reportToHost({ ok: true, adapterInfo: ADAPTER }, bridge);
        expect(bridge.reports).toEqual([{ ok: true, adapter: ADAPTER }]);
    });

    it.each<HostVerdict>([
        { verdict: 'continue' },
        { verdict: 'relaunching' },
        { verdict: 'exhausted', diagnostics: ['profile: vulkan'] },
    ])('passes the host verdict through: $verdict', async (verdict) => {
        expect(await reportToHost({ ok: false, reason: 'no-webgpu' }, fakeBridge(verdict))).toEqual(verdict);
    });

    it('carries on when the host does not answer', async () => {
        const error = vi.spyOn(console, 'error').mockImplementation(() => {});
        const bridge: ElectronGpuBridge = { report: () => Promise.reject(new Error('no handler')) };
        expect(await reportToHost({ ok: false, reason: 'no-adapter' }, bridge)).toEqual({ verdict: 'continue' });
        error.mockRestore();
    });
});
