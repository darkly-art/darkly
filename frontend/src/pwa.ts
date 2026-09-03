import { registerSW } from 'virtual:pwa-register';
import { toast } from './state/toast.svelte';

/**
 * Register the service worker and surface a non-interrupting reload prompt when
 * a new build is waiting. We use `registerType: 'prompt'` (see vite.config.ts)
 * so an in-progress stroke is never reloaded out from under the artist; they
 * click "Reload" when ready, which activates the waiting SW and reloads.
 */
export function registerPwa() {
    const updateSW = registerSW({
        onNeedRefresh() {
            toast.show('info', 'New version available', {
                sticky: true,
                action: {
                    label: 'Reload',
                    onClick: () => updateSW(true),
                },
            });
        },
    });
}
