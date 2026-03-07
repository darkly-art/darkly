import { app } from '../state/app.svelte';
import { config } from '../config/store.svelte';

type NavMode = 'none' | 'pan' | 'rotate' | 'zoom';

// 24x24 rotate cursor (circular arrows), white stroke with dark outline for visibility
const ROTATE_SVG = `<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none"><path d="M21 2v6h-6M3 22v-6h6M21 8A9 9 0 0 0 6 3.3L3 6M3 16a9 9 0 0 0 15 4.7l3-2.7" stroke="#000" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/><path d="M21 2v6h-6M3 22v-6h6M21 8A9 9 0 0 0 6 3.3L3 6M3 16a9 9 0 0 0 15 4.7l3-2.7" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>`;
const ROTATE_CURSOR = `url("data:image/svg+xml,${encodeURIComponent(ROTATE_SVG)}") 12 12, auto`;

/** Map modifier name to the corresponding PointerEvent/WheelEvent flag. */
function hasModifier(e: PointerEvent | WheelEvent, mod: string): boolean {
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
        if (e.code === config.get('hotkeys.nav.trigger') && !e.repeat) {
            e.preventDefault();
            this.spaceHeld = true;
        }
    }

    onKeyUp(e: KeyboardEvent) {
        if (e.code === config.get('hotkeys.nav.trigger')) {
            this.spaceHeld = false;
            this.mode = 'none';
        }
    }

    onPointerDown(e: PointerEvent, canvasEl?: HTMLCanvasElement): boolean {
        if (!this.spaceHeld) return false;

        const zoom = config.get('hotkeys.nav.zoom') as string;
        const rotate = config.get('hotkeys.nav.rotate') as string;

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

    // --- Touch gesture handling ---

    /** Active touch pointers for multi-finger gesture detection. */
    private touches = new Map<number, { x: number; y: number }>();

    /** Set when 2+ fingers are detected; stays true until all fingers lift. */
    private touchGestureOccurred = false;

    /** Whether a multi-finger touch gesture is in progress. */
    get isTouchGesture(): boolean {
        return this.touchGestureOccurred;
    }

    /**
     * Track a touch pointer down. Returns true if a two-finger gesture is
     * now active (event should not be dispatched to tools).
     */
    onTouchPointerDown(e: PointerEvent): boolean {
        this.touches.set(e.pointerId, { x: e.clientX, y: e.clientY });
        if (this.touches.size >= 2) {
            this.touchGestureOccurred = true;
        }
        return this.touchGestureOccurred;
    }

    /**
     * Update a touch pointer position and, if two fingers are active,
     * apply the incremental pan/zoom/rotation gesture.
     */
    onTouchPointerMove(e: PointerEvent, canvasEl: HTMLCanvasElement) {
        if (!this.touches.has(e.pointerId)) return;

        if (this.touches.size < 2) {
            // Not in gesture — just keep position current for when/if a
            // second finger arrives.
            this.touches.set(e.pointerId, { x: e.clientX, y: e.clientY });
            return;
        }

        // Snapshot previous two-finger state
        const entries = [...this.touches.entries()];
        const [id1, prev1] = entries[0];
        const [, prev2] = entries[1];
        const prevCx = (prev1.x + prev2.x) / 2;
        const prevCy = (prev1.y + prev2.y) / 2;
        const prevDist = Math.hypot(prev2.x - prev1.x, prev2.y - prev1.y);
        const prevAngle = Math.atan2(prev2.y - prev1.y, prev2.x - prev1.x);

        // Update the moved pointer
        this.touches.set(e.pointerId, { x: e.clientX, y: e.clientY });

        // Compute current two-finger state
        const cur1 = this.touches.get(id1)!;
        const cur2 = this.touches.get(entries[1][0])!;
        const curCx = (cur1.x + cur2.x) / 2;
        const curCy = (cur1.y + cur2.y) / 2;
        const curDist = Math.hypot(cur2.x - cur1.x, cur2.y - cur1.y);
        const curAngle = Math.atan2(cur2.y - cur1.y, cur2.x - cur1.x);

        // Pan: delta of midpoints
        app.panX += curCx - prevCx;
        app.panY += curCy - prevCy;

        // Zoom: centered on gesture midpoint
        if (prevDist > 1) {
            const zoomFactor = curDist / prevDist;
            const newZoom = Math.max(0.01, Math.min(100, app.zoom * zoomFactor));

            const rect = canvasEl.getBoundingClientRect();
            const cursorX = curCx - rect.left;
            const cursorY = curCy - rect.top;
            const pivotX = rect.width / 2 + app.panX;
            const pivotY = rect.height / 2 + app.panY;
            const ratio = 1 - newZoom / app.zoom;
            app.panX += (cursorX - pivotX) * ratio;
            app.panY += (cursorY - pivotY) * ratio;
            app.zoom = newZoom;
        }

        // Rotation: angular delta
        app.rotation -= curAngle - prevAngle;
    }

    onTouchPointerUp(e: PointerEvent) {
        this.touches.delete(e.pointerId);
        if (this.touches.size === 0) {
            this.touchGestureOccurred = false;
        }
    }

    // --- Wheel handling (scroll + zoom) ---

    onWheel(e: WheelEvent, canvasEl: HTMLCanvasElement) {
        e.preventDefault();

        // Normalize for different deltaMode values (lines vs pixels)
        const scale = e.deltaMode === 1 ? 16 : 1;
        const deltaX = e.deltaX * scale;
        const deltaY = e.deltaY * scale;

        if (hasModifier(e, config.get('hotkeys.nav.zoom') as string)) {
            // Zoom (Ctrl+scroll, or trackpad pinch which fires ctrlKey=true)
            const factor = Math.pow(1.001, -deltaY);
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
        } else {
            // Pan (two-finger scroll on trackpad, or mouse scroll wheel)
            app.panX -= deltaX;
            app.panY -= deltaY;
        }
    }
}

export const nav = new NavigationState();
