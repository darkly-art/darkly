import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import {
    afterAdapterCheck,
    chooseProfile,
    countGpus,
    launchDiagnostics,
    pinningEnv,
    readLaunchState,
    writeLaunchState,
    type LaunchEnv,
} from '../gpu-launch';

const CHROME = '152.0.7977.130';
const WAYLAND: LaunchEnv = { WAYLAND_DISPLAY: 'wayland-1' };
const X11: LaunchEnv = {};

describe('regressions', () => {
    it('a Wayland session starts on Chromium defaults', () => {
        expect(chooseProfile(WAYLAND, CHROME, null).profile.switches).toEqual([]);
    });

    it('a hybrid laptop pins every Vulkan user to the display GPU', () => {
        expect(pinningEnv({}, 2)).toEqual({ MESA_VK_DEVICE_SELECT_FORCE_DEFAULT_DEVICE: '1' });
    });

    it("an artist's DRI_PRIME is respected", () => {
        expect(pinningEnv({ DRI_PRIME: '1' }, 2)).toEqual({});
    });
});

describe('chooseProfile', () => {
    it('starts an X11 session on the Vulkan compositor', () => {
        const { profile } = chooseProfile(X11, CHROME, null);
        expect(profile.id).toBe('vulkan');
        expect(profile.switches).toEqual([['enable-features', 'Vulkan']]);
    });

    it('honours a stored profile for the same Chromium and session', () => {
        const choice = chooseProfile(WAYLAND, CHROME, { chrome: CHROME, session: 'wayland', profile: 'vulkan' });
        expect(choice.profile.id).toBe('vulkan');
        expect(choice.state).toEqual({ chrome: CHROME, session: 'wayland', profile: 'vulkan' });
    });

    it('ignores a stored profile from another Chromium or session', () => {
        expect(chooseProfile(WAYLAND, CHROME, { chrome: '151.0', session: 'wayland', profile: 'vulkan' }).profile.id)
            .toBe('defaults');
        expect(chooseProfile(WAYLAND, CHROME, { chrome: CHROME, session: 'x11', profile: 'vulkan' }).profile.id)
            .toBe('defaults');
    });

    it('ignores a stored profile that cannot work in this session', () => {
        expect(chooseProfile(X11, CHROME, { chrome: CHROME, session: 'x11', profile: 'defaults' }).profile.id)
            .toBe('vulkan');
    });

    it('lets DARKLY_GPU_PROFILE win, pinned, without changing what is remembered', () => {
        const stored = { chrome: CHROME, session: 'wayland' as const, profile: 'defaults' as const };
        const choice = chooseProfile({ ...WAYLAND, DARKLY_GPU_PROFILE: 'vulkan' }, CHROME, stored);
        expect(choice.profile.id).toBe('vulkan');
        expect(choice.pinned).toBe(true);
        expect(choice.state).toEqual(stored);
    });

    it('ignores an unknown DARKLY_GPU_PROFILE with a warning', () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        const choice = chooseProfile({ ...WAYLAND, DARKLY_GPU_PROFILE: 'opengl' }, CHROME, null);
        expect(choice.profile.id).toBe('defaults');
        expect(choice.pinned).toBe(false);
        expect(warn).toHaveBeenCalledOnce();
        warn.mockRestore();
    });
});

describe('afterAdapterCheck', () => {
    it('continues when the renderer has an adapter', () => {
        expect(afterAdapterCheck(WAYLAND, chooseProfile(WAYLAND, CHROME, null), true)).toEqual({ verdict: 'continue' });
    });

    it('relaunches from defaults to vulkan', () => {
        expect(afterAdapterCheck(WAYLAND, chooseProfile(WAYLAND, CHROME, null), false)).toEqual({
            verdict: 'relaunch',
            store: { chrome: CHROME, session: 'wayland', profile: 'vulkan' },
        });
    });

    it('is exhausted past the last profile and resets the stored one to the first', () => {
        const onVulkan = chooseProfile(WAYLAND, CHROME, { chrome: CHROME, session: 'wayland', profile: 'vulkan' });
        expect(afterAdapterCheck(WAYLAND, onVulkan, false)).toEqual({
            verdict: 'exhausted',
            store: { chrome: CHROME, session: 'wayland', profile: 'defaults' },
        });
    });

    it('never relaunches a pinned profile', () => {
        const pinned = chooseProfile({ ...WAYLAND, DARKLY_GPU_PROFILE: 'defaults' }, CHROME, null);
        expect(afterAdapterCheck(WAYLAND, pinned, false).verdict).toBe('exhausted');
    });
});

describe('pinningEnv', () => {
    it('sets nothing on a one-GPU machine', () => {
        expect(pinningEnv({}, 1)).toEqual({});
        expect(pinningEnv({}, 0)).toEqual({});
    });

    it.each([
        'DRI_PRIME',
        'MESA_VK_DEVICE_SELECT',
        'MESA_VK_DEVICE_SELECT_FORCE_DEFAULT_DEVICE',
        'NODEVICE_SELECT',
        'VK_LOADER_DRIVERS_DISABLE',
    ] as const)('sets nothing when %s is present, even empty', (knob) => {
        expect(pinningEnv({ [knob]: '' }, 2)).toEqual({});
        expect(pinningEnv({ [knob]: '1' }, 2)).toEqual({});
    });
});

describe('countGpus', () => {
    const devices: Record<string, string> = {
        card0: '/sys/devices/pci0000:00/0000:00:02.0',
        card1: '/sys/devices/pci0000:00/0000:00:01.0/0000:01:00.0',
        card2: '/sys/devices/pci0000:00/0000:00:02.0',
    };
    const resolve = (card: string) => {
        if (!(card in devices)) throw new Error('no device link');
        return devices[card];
    };

    it('counts two cards on one device once', () => {
        expect(countGpus(['card0', 'card2'], resolve)).toBe(1);
    });

    it('counts two devices twice, skipping connectors and render nodes', () => {
        expect(countGpus(['card0', 'card0-eDP-1', 'card1', 'card1-DP-1', 'renderD128', 'version'], resolve)).toBe(2);
    });

    it('counts nothing with no cards, or cards without a device', () => {
        expect(countGpus([], resolve)).toBe(0);
        expect(countGpus(['card9'], resolve)).toBe(0);
    });
});

describe('launch state file', () => {
    let dir: string;
    beforeEach(() => {
        dir = fs.mkdtempSync(path.join(os.tmpdir(), 'darkly-gpu-launch-'));
    });
    afterEach(() => {
        fs.rmSync(dir, { recursive: true, force: true });
    });

    it('round-trips', () => {
        const file = path.join(dir, 'nested', 'gpu-launch.json');
        const state = { chrome: CHROME, session: 'x11' as const, profile: 'vulkan' as const };
        writeLaunchState(file, state);
        expect(readLaunchState(file)).toEqual(state);
    });

    it('reads a missing or corrupt file as none', () => {
        const file = path.join(dir, 'gpu-launch.json');
        expect(readLaunchState(file)).toBeNull();
        fs.writeFileSync(file, '{not json');
        expect(readLaunchState(file)).toBeNull();
        fs.writeFileSync(file, '"152"');
        expect(readLaunchState(file)).toBeNull();
    });
});

describe('launchDiagnostics', () => {
    it('names the profile, session, renderer failure and every device knob', () => {
        const lines = launchDiagnostics(
            { ...WAYLAND, DRI_PRIME: '1' },
            chooseProfile(WAYLAND, CHROME, null),
            2,
            { ok: false, reason: 'no-adapter' },
        );
        expect(lines).toEqual(expect.arrayContaining([
            'profile: defaults',
            'session: wayland (WAYLAND_DISPLAY=wayland-1)',
            `chromium: ${CHROME}`,
            'gpus: 2',
            'renderer: no-adapter',
            'DRI_PRIME=1',
            'NODEVICE_SELECT=(unset)',
        ]));
    });
});
