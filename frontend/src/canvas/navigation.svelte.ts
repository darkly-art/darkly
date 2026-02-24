import { app } from '../state/app.svelte';
import { user } from '../config/store.svelte';

type NavMode = 'none' | 'pan' | 'rotate' | 'zoom';

// 24x24 rotate cursor (circular arrows), white stroke with dark outline for visibility
const ROTATE_SVG = `<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none"><path d="M21 2v6h-6M3 22v-6h6M21 8A9 9 0 0 0 6 3.3L3 6M3 16a9 9 0 0 0 15 4.7l3-2.7" stroke="#000" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/><path d="M21 2v6h-6M3 22v-6h6M21 8A9 9 0 0 0 6 3.3L3 6M3 16a9 9 0 0 0 15 4.7l3-2.7" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>`;
const ROTATE_CURSOR = `url("data:image/svg+xml,${encodeURIComponent(ROTATE_SVG)}") 12 12, auto`;

/** Map modifier name to the corresponding PointerEvent/WheelEvent flag. */
function hasModifier(e: PointerEvent | WheelEvent, mod: 'Shift' | 'Ctrl' | 'Alt'): boolean {
    if (mod === 'Shift') return e.shiftKey;
    if (mod === 'Ctrl') return e.ctrlKey;
    return e.altKey;
}

class NavigationState {
    mode = $state<NavMode>('none');
    private startX = 0;
    private startY = 0;
    private startPanX = 0;
    private startPanY = 0;
    private startRotation = 0;
    private startZoom = 0;
    private startAngle = 0;
    private centerX = 0;
    private centerY = 0;

    /** Track whether the navigation trigger key is held */
    spaceHeld = $state(false);

    get isNavigating(): boolean {
        return this.mode !== 'none';
    }

    get cursor(): string {
        switch (this.mode) {
            case 'pan': return 'grabbing';
            case 'zoom': return 'zoom-in';
            case 'rotate': return ROTATE_CURSOR;
            default: return this.spaceHeld ? 'grab' : 'crosshair';
        }
    }

    onKeyDown(e: KeyboardEvent) {
        if (e.code === user.resolved.hotkeys.nav.trigger && !e.repeat) {
            e.preventDefault();
            this.spaceHeld = true;
        }
    }

    onKeyUp(e: KeyboardEvent) {
        if (e.code === user.resolved.hotkeys.nav.trigger) {
            this.spaceHeld = false;
            this.mode = 'none';
        }
    }

    onPointerDown(e: PointerEvent, canvasEl?: HTMLCanvasElement): boolean {
        if (!this.spaceHeld) return false;

        const { zoom, rotate } = user.resolved.hotkeys.nav;

        if (hasModifier(e, zoom)) {
            this.mode = 'zoom';
        } else if (hasModifier(e, rotate)) {
            this.mode = 'rotate';
        } else {
            this.mode = 'pan';
        }

        this.startX = e.clientX;
        this.startY = e.clientY;
        this.startPanX = app.panX;
        this.startPanY = app.panY;
        this.startRotation = app.rotation;
        this.startZoom = app.zoom;

        // For Krita-style angular rotation: measure angle from the on-screen
        // position of the canvas center, which is element-center + pan.
        if (this.mode === 'rotate' && canvasEl) {
            const rect = canvasEl.getBoundingClientRect();
            this.centerX = rect.left + rect.width / 2 + app.panX;
            this.centerY = rect.top + rect.height / 2 + app.panY;
            this.startAngle = Math.atan2(
                e.clientY - this.centerY,
                e.clientX - this.centerX,
            );
        }

        return true; // consumed — don't dispatch to tool
    }

    onPointerMove(e: PointerEvent) {
        if (this.mode === 'none') return;

        const dx = e.clientX - this.startX;
        const dy = e.clientY - this.startY;

        switch (this.mode) {
            case 'pan':
                app.panX = this.startPanX + dx;
                app.panY = this.startPanY + dy;
                break;
            case 'rotate': {
                // Krita-style: angular rotation around the canvas center.
                const curAngle = Math.atan2(
                    e.clientY - this.centerY,
                    e.clientX - this.centerX,
                );
                app.rotation = this.startRotation - (curAngle - this.startAngle);
            }
                break;
            case 'zoom':
                // Drag down = zoom in, drag up = zoom out. Exponential scaling.
                app.zoom = this.startZoom * Math.pow(2, -dy / 150);
                break;
        }
    }

    onPointerUp() {
        this.mode = 'none';
    }

    onWheel(e: WheelEvent, canvasEl: HTMLCanvasElement) {
        // Scroll zoom uses the same modifier as drag zoom (Ctrl by default).
        if (!hasModifier(e, user.resolved.hotkeys.nav.zoom)) return;

        e.preventDefault();
        const factor = Math.pow(1.001, -e.deltaY);
        const newZoom = Math.max(0.01, Math.min(100, app.zoom * factor));

        // Zoom centered on cursor: keep the point under the cursor fixed.
        const rect = canvasEl.getBoundingClientRect();
        const cursorX = e.clientX - rect.left;
        const cursorY = e.clientY - rect.top;
        const pivotX = rect.width / 2 + app.panX;
        const pivotY = rect.height / 2 + app.panY;
        const ratio = 1 - newZoom / app.zoom;

        app.panX += (cursorX - pivotX) * ratio;
        app.panY += (cursorY - pivotY) * ratio;
        app.zoom = newZoom;
    }
}

export const nav = new NavigationState();
