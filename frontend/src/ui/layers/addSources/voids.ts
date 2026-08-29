import { app } from '../../../state/app.svelte';
import type { CaptureKind } from '../../../lib/frameSource';
import type { AddSource } from './types';

/**
 * A void — a layer filled from a procedural or live source.
 *
 * Spawning lives here rather than in the modal because of the acquisition
 * ordering below: no shared spawn path could hold it, since it constrains what
 * may be awaited before what.
 */
export const source: AddSource = {
    action: 'newVoid',
    catalog: 'voids',
    async spawn(entry) {
        if (!app.engine) return;
        // For MediaStream-backed voids (camera / screenshare), acquire the
        // MediaStream IN this click gesture, BEFORE the awaitable `add_void`
        // round-trip. `getDisplayMedia` requires transient user activation,
        // which would expire if we acquired only after awaiting add_void. If
        // the user cancels / denies, we still create the layer and record the
        // error so the properties panel can offer Resume. A `stream` void
        // (Blender) needs no gesture or permission — it connects over localhost
        // HTTP after the layer exists — so skip acquisition entirely.
        const captureKind: CaptureKind | undefined = entry.captureKind ?? undefined;
        let stream: MediaStream | undefined;
        let acquireError: unknown;
        if (captureKind === 'camera' || captureKind === 'display') {
            try {
                stream = await app.acquireMediaStream(captureKind);
            } catch (err) {
                acquireError = err;
            }
        }

        const defaults: Record<string, any> = {};
        for (const p of entry.params) {
            defaults[p.name] = p.default;
        }
        const id = await app.engine.api.addVoid({
            void_type: entry.type,
            params: defaults,
            anchor: app.activeLayerId,
        });
        if (id != null) {
            app.selectLayer(id);
            // Adding a stream-backed void via the picker is an explicit user
            // gesture — opt the new layer into this session's allow-list and
            // hand it the pre-acquired stream (or the acquire error). Reopening
            // a saved doc does NOT add to this set, which is why loaded
            // stream voids hold their saved frame until the user clicks Resume.
            if (captureKind) {
                app.markStreamVoidStarted(id);
                await app.startStreamSource(id, captureKind, stream, acquireError);
            }
        } else if (stream) {
            // Layer creation failed but we acquired a stream — release it so the
            // OS capture indicator doesn't linger.
            stream.getTracks().forEach((t) => t.stop());
        }
        app.requestFrame();
    },
};
