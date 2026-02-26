export type ToastLevel = 'success' | 'info' | 'warning' | 'error';

interface Toast {
    id: number;
    level: ToastLevel;
    message: string;
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

    show(level: ToastLevel, message: string, durationMs?: number) {
        const id = nextId++;
        this.toasts.push({ id, level, message });
        const ms = durationMs ?? AUTO_DISMISS_MS[level];
        setTimeout(() => this.dismiss(id), ms);
    }

    dismiss(id: number) {
        this.toasts = this.toasts.filter(t => t.id !== id);
    }
}

export const toast = new ToastState();
