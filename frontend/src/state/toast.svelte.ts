export type ToastLevel = 'success' | 'info' | 'warning' | 'error';

/** Optional actionable button rendered inside a toast (e.g. "Reload"). */
export interface ToastAction {
    label: string;
    onClick: () => void;
}

export interface ToastOptions {
    /** Override the level's default auto-dismiss delay. */
    durationMs?: number;
    /** Keep the toast up until dismissed (or its action is taken). */
    sticky?: boolean;
    /** Render a button that runs `onClick` when pressed. */
    action?: ToastAction;
}

interface Toast {
    id: number;
    level: ToastLevel;
    message: string;
    action?: ToastAction;
}

const AUTO_DISMISS_MS: Record<ToastLevel, number> = {
    success: 2000,
    info: 2000,
    warning: 2000,
    error: 3000,
};

let nextId = 1;

class ToastState {
    toasts = $state<Toast[]>([]);

    show(level: ToastLevel, message: string, opts: ToastOptions = {}) {
        // A repeat replaces its predecessor rather than stacking beside it:
        // clicking three times on a layer that refuses paint is one complaint,
        // not three. Dropping the old entry also restarts the dismiss delay,
        // so the message stays up as long as the artist keeps provoking it.
        const dup = this.toasts.find((t) => t.level === level && t.message === message);
        if (dup) this.dismiss(dup.id);

        const id = nextId++;
        this.toasts.push({ id, level, message, action: opts.action });
        // Sticky toasts (e.g. the update prompt) persist until the artist acts.
        if (!opts.sticky) {
            const ms = opts.durationMs ?? AUTO_DISMISS_MS[level];
            setTimeout(() => this.dismiss(id), ms);
        }
    }

    dismiss(id: number) {
        this.toasts = this.toasts.filter(t => t.id !== id);
    }
}

export const toast = new ToastState();
