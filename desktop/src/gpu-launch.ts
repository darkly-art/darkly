/**
 * How the desktop app brings WebGPU up on Linux.
 *
 * No single Chromium flag set works on every Linux machine (Chromium
 * 152.0.7977.130 sources):
 *
 * - Chromium's default path is WebGPU-on-Vulkan-via-GL-interop, which only the
 *   Wayland Ozone backend enables (gpu_init.cc:724-731,
 *   ozone_platform_wayland.cc:372), and which blocklist entry 186
 *   (software_rendering_list.json) allows only on Intel gen 12+ with Mesa 22+
 *   and NVIDIA 535+. Elsewhere Dawn gets no backend and requestAdapter() is
 *   null (webgpu_decoder_impl.cc:1750-1758).
 * - The Vulkan compositor (`--enable-features=Vulkan`) works on X11, AMD and
 *   older Intel, but moves Skia and Viz off GL on machines that did not need it.
 *
 * So launch profiles are data: the host starts at the first profile that
 * applies to the session, the renderer reports whether it got an adapter, and
 * on failure the host stores the next profile and relaunches once, the same
 * ordered, bounded fallback as Chromium's own
 * GpuDataManagerImplPrivate::FallBackToNextGpuMode.
 *
 * Separately, on a machine with more than one GPU, Mesa's device-select layer
 * is told to expose only the default device to every Vulkan instance in the
 * GPU process (device_select_layer.c:337-339). Without it a hybrid laptop's
 * Vulkan compositor and Dawn both land on the discrete GPU
 * (vulkan_device_queue.cc:191) while the display belongs to the integrated
 * one, and the canvas is black. Chromium's instances resolve the default
 * through the Wayland compositor's main device, Dawn's through boot_vga
 * (device_select.c:325-369); on a muxless laptop both are the integrated GPU.
 * The variable has to be in the environment the GPU process inherits, which
 * Chromium's zygote fixes before any JavaScript runs, so it is delivered by a
 * relaunch before `ready`.
 *
 * Kept apart from main.ts, which touches Electron's `app` singleton at import
 * time and so cannot be loaded by a test.
 */
import * as fs from 'fs';
import * as path from 'path';

/** The subset of process.env the launch policy reads. */
export interface LaunchEnv {
    WAYLAND_DISPLAY?: string;
    DARKLY_GPU_PROFILE?: string;
    DRI_PRIME?: string;
    MESA_VK_DEVICE_SELECT?: string;
    MESA_VK_DEVICE_SELECT_FORCE_DEFAULT_DEVICE?: string;
    NODEVICE_SELECT?: string;
    VK_LOADER_DRIVERS_DISABLE?: string;
}

export type ProfileId = 'defaults' | 'vulkan';
export type Session = 'wayland' | 'x11';

export interface LaunchProfile {
    id: ProfileId;
    /** Chromium switches for app.commandLine.appendSwitch. */
    switches: ReadonlyArray<readonly [name: string, value?: string]>;
    /** Can this profile work in this session at all? */
    applies(env: LaunchEnv): boolean;
}

export const LINUX_PROFILES: readonly LaunchProfile[] = [
    // Chromium's own defaults: WebGPU-on-Vulkan-via-GL-interop. Wayland only;
    // the X11 backend never enables interop, so there it is a guaranteed null
    // adapter.
    { id: 'defaults', switches: [], applies: (e) => !!e.WAYLAND_DISPLAY },
    // The Vulkan compositor: X11, AMD, Intel before gen 12.
    { id: 'vulkan', switches: [['enable-features', 'Vulkan']], applies: () => true },
];

export function applicableProfiles(
    env: LaunchEnv,
    profiles: readonly LaunchProfile[] = LINUX_PROFILES,
): LaunchProfile[] {
    return profiles.filter((p) => p.applies(env));
}

export function sessionOf(env: LaunchEnv): Session {
    return env.WAYLAND_DISPLAY ? 'wayland' : 'x11';
}

/**
 * What the host remembers between launches, in userData. `chrome` keys the GPU
 * cache purge and the profile alike: both are valid only for the Chromium
 * that wrote them. `session` and `profile` are the Linux profile last chosen
 * for that session, absent on other platforms.
 */
export interface LaunchState {
    chrome: string;
    session?: Session;
    profile?: ProfileId;
}

export interface ProfileChoice {
    /** The profile this launch runs. */
    profile: LaunchProfile;
    /** What to store for later launches. A pinned launch keeps the profile
     *  they remember rather than its own. */
    state: LaunchState;
    /** The artist named the profile; never relaunch away from it. */
    pinned: boolean;
}

export function chooseProfile(
    env: LaunchEnv,
    chrome: string,
    stored: LaunchState | null,
    profiles: readonly LaunchProfile[] = LINUX_PROFILES,
): ProfileChoice {
    const session = sessionOf(env);
    const applicable = applicableProfiles(env, profiles);
    const remembered =
        stored?.chrome === chrome && stored.session === session
            ? applicable.find((p) => p.id === stored.profile)
            : undefined;

    let named: LaunchProfile | undefined;
    if (env.DARKLY_GPU_PROFILE) {
        named = profiles.find((p) => p.id === env.DARKLY_GPU_PROFILE);
        if (!named) {
            console.warn(
                `Darkly GPU launch: ignoring unknown DARKLY_GPU_PROFILE=${env.DARKLY_GPU_PROFILE} ` +
                    `(known: ${profiles.map((p) => p.id).join(', ')})`,
            );
        }
    }

    const unpinned = remembered ?? applicable[0];
    return {
        profile: named ?? unpinned,
        state: { chrome, session, profile: named ? remembered?.id : unpinned.id },
        pinned: named !== undefined,
    };
}

export type Outcome =
    | { verdict: 'continue' }
    | { verdict: 'relaunch'; store: LaunchState }
    | { verdict: 'exhausted'; store: LaunchState };

/**
 * The host's answer to the renderer's adapter check. On failure, move to the
 * next applicable profile; past the last one, give up and reset the stored
 * profile to the first, so one launch chain relaunches at most
 * `applicable.length - 1` times and the next manual launch starts clean.
 */
export function afterAdapterCheck(
    env: LaunchEnv,
    choice: ProfileChoice,
    hasAdapter: boolean,
    profiles: readonly LaunchProfile[] = LINUX_PROFILES,
): Outcome {
    if (hasAdapter) return { verdict: 'continue' };
    if (choice.pinned) return { verdict: 'exhausted', store: choice.state };

    const applicable = applicableProfiles(env, profiles);
    const next = applicable[applicable.indexOf(choice.profile) + 1];
    if (next !== undefined) return { verdict: 'relaunch', store: { ...choice.state, profile: next.id } };
    return { verdict: 'exhausted', store: { ...choice.state, profile: applicable[0].id } };
}

/** Read the stored state; anything missing or malformed reads as none. */
export function readLaunchState(file: string): LaunchState | null {
    try {
        const parsed: unknown = JSON.parse(fs.readFileSync(file, 'utf8'));
        if (typeof parsed === 'object' && parsed !== null && typeof (parsed as LaunchState).chrome === 'string') {
            return parsed as LaunchState;
        }
    } catch {
        // Missing or unreadable: same as never written.
    }
    return null;
}

export function writeLaunchState(file: string, state: LaunchState): void {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, JSON.stringify(state));
}

/** Every way an artist can already steer Vulkan device selection. If any is
 *  present, even empty, Darkly leaves device selection alone. */
const DEVICE_KNOBS = [
    'DRI_PRIME',
    'MESA_VK_DEVICE_SELECT',
    'MESA_VK_DEVICE_SELECT_FORCE_DEFAULT_DEVICE',
    'NODEVICE_SELECT',
    'VK_LOADER_DRIVERS_DISABLE',
] as const satisfies ReadonlyArray<keyof LaunchEnv>;

/** Environment to add before the GPU process starts: empty on a one-GPU
 *  machine or when the artist steers device selection themselves. */
export function pinningEnv(env: LaunchEnv, gpuCount: number): Record<string, string> {
    if (gpuCount < 2 || DEVICE_KNOBS.some((k) => env[k] !== undefined)) return {};
    return { MESA_VK_DEVICE_SELECT_FORCE_DEFAULT_DEVICE: '1' };
}

/** Distinct devices behind the /sys/class/drm `card<N>` entries (connector
 *  entries such as `card1-eDP-1` are skipped), given the directory listing
 *  and a resolver from a card's `device` link to its real path. */
export function countGpus(entries: string[], resolveDevice: (card: string) => string): number {
    const devices = new Set<string>();
    for (const entry of entries) {
        if (!/^card\d+$/.test(entry)) continue;
        try {
            devices.add(resolveDevice(entry));
        } catch {
            // A card with no device link describes no GPU.
        }
    }
    return devices.size;
}

/** countGpus over this machine's /sys/class/drm; zero where it is unreadable. */
export function systemGpuCount(drm = '/sys/class/drm'): number {
    let entries: string[];
    try {
        entries = fs.readdirSync(drm);
    } catch {
        return 0;
    }
    return countGpus(entries, (card) => fs.realpathSync(path.join(drm, card, 'device')));
}

/** Shape the renderer sends on `gpu:report` (frontend/src/gpu.ts GpuReport). */
export type GpuReport =
    | { ok: false; reason: string }
    | { ok: true; adapter: { vendor: string; architecture: string; device: string; description: string } };

/** What the exhausted error page shows, for a bug report. */
export function launchDiagnostics(
    env: LaunchEnv,
    choice: ProfileChoice,
    gpuCount: number,
    report: GpuReport,
): string[] {
    const shown = (k: keyof LaunchEnv) => `${k}=${env[k] ?? '(unset)'}`;
    return [
        `profile: ${choice.profile.id}${choice.pinned ? ' (DARKLY_GPU_PROFILE)' : ''}`,
        `session: ${sessionOf(env)} (${shown('WAYLAND_DISPLAY')})`,
        `chromium: ${choice.state.chrome}`,
        `gpus: ${gpuCount}`,
        `renderer: ${report.ok ? 'adapter' : report.reason}`,
        ...DEVICE_KNOBS.map(shown),
    ];
}
