/**
 * TEMPORARY — soft-contrast overlay iteration tool.
 *
 * Hover to see a masked stamp drawn with FLAG_SOFT_CONTRAST on top of the
 * underlying canvas. The stamp shape + softness come entirely from the mask
 * texture (same model as the brush: grayscale tip, red channel = coverage).
 *
 * Keys:
 *   [  / ]   — decrease / increase tint strength by 0.02
 *   -  / =   — decrease / increase radius by 4 canvas px
 *   ,  / .   — rotate stamp -15° / +15°
 *   h        — cycle mask (soft radial → hard round → streaky)
 *   0        — reset strength, radius, rotation to defaults
 *
 * Values are logged to the console on change. Delete this file, its entry in
 * index.ts, and the KIND_MASKED_STAMP/FLAG_SOFT_CONTRAST exports in
 * selection_helpers.ts once the look is locked in and brush wiring begins.
 */
import type { Tool } from './registry';
import { app } from '../state/app.svelte';
import {
    KIND_MASKED_STAMP,
    FLAG_CANVAS_SPACE,
    FLAG_SOFT_CONTRAST,
    prim,
} from './selection_helpers';

let strength = 0.22;
let radius = 40;
let rotationRad = 0;
let maskIndex = 0;
let lastPos: [number, number] | null = null;
let maskUploaded = false;

const MASK_NAMES = ['soft-radial', 'hard-round', 'streaky'] as const;

/** Generate an RGBA8 mask. Red channel is what the shader samples. */
function generateMask(kind: typeof MASK_NAMES[number], size = 128): Uint8Array {
    const data = new Uint8Array(size * size * 4);
    const half = size / 2;
    for (let y = 0; y < size; y++) {
        for (let x = 0; x < size; x++) {
            const dx = (x - half) / half;
            const dy = (y - half) / half;
            const d = Math.hypot(dx, dy);
            let v = 0;
            if (kind === 'soft-radial') {
                // Gaussian-ish falloff; zero at the disc edge.
                v = d >= 1 ? 0 : Math.pow(Math.cos(d * Math.PI * 0.5), 2);
            } else if (kind === 'hard-round') {
                v = d <= 1 ? 1 : 0;
            } else { // streaky
                // Horizontal gaussian streak, vertical square disc mask.
                const axial = Math.exp(-dy * dy * 6);
                const disc = d <= 1 ? 1 : 0;
                v = axial * disc;
            }
            const b = Math.round(Math.max(0, Math.min(1, v)) * 255);
            const i = (y * size + x) * 4;
            data[i] = b; data[i+1] = b; data[i+2] = b; data[i+3] = 255;
        }
    }
    return data;
}

function uploadCurrentMask() {
    if (!app.handle) return;
    const size = 128;
    const data = generateMask(MASK_NAMES[maskIndex], size);
    app.handle.set_overlay_mask(size, size, data);
    maskUploaded = true;
}

function pushPreview() {
    if (!app.handle || !lastPos) return;
    app.handle.set_overlay([
        prim(
            KIND_MASKED_STAMP,
            FLAG_CANVAS_SPACE | FLAG_SOFT_CONTRAST,
            lastPos,
            [radius, radius],
            { modeParam: strength, rotation: rotationRad },
        ),
    ]);
}

function logParams() {
    const deg = ((rotationRad * 180) / Math.PI).toFixed(0);
    console.log(
        `[soft-overlay] mask=${MASK_NAMES[maskIndex]} strength=${strength.toFixed(2)}`
        + ` radius=${radius} rotation=${deg}°`,
    );
}

export const overlayPreviewTool: Tool = {
    id: 'overlay_preview',
    name: 'Soft Overlay Preview (dev)',
    faIcon: 'fa-solid fa-flask',
    group: 'paint',
    hotkeyAction: 'overlayPreviewTool',

    onActivate() {
        if (!maskUploaded) uploadCurrentMask();
        logParams();
    },

    onDeactivate() {
        lastPos = null;
        app.handle?.clear_overlay();
    },

    onPointerDown(_ctx, _e, cx, cy) {
        lastPos = [cx, cy];
        pushPreview();
    },

    onPointerMove(_ctx, _e, cx, cy) {
        lastPos = [cx, cy];
        pushPreview();
    },

    onPointerUp() {},

    onKeyDown(e) {
        let changed = false;
        let maskChanged = false;
        if (e.key === '[') { strength = Math.max(0, strength - 0.02); changed = true; }
        else if (e.key === ']') { strength = Math.min(1, strength + 0.02); changed = true; }
        else if (e.key === '-') { radius = Math.max(4, radius - 4); changed = true; }
        else if (e.key === '=') { radius = radius + 4; changed = true; }
        else if (e.key === ',') { rotationRad -= Math.PI / 12; changed = true; }
        else if (e.key === '.') { rotationRad += Math.PI / 12; changed = true; }
        else if (e.key === 'h') {
            maskIndex = (maskIndex + 1) % MASK_NAMES.length;
            uploadCurrentMask();
            maskChanged = true;
            changed = true;
        }
        else if (e.key === '0') {
            strength = 0.22; radius = 40; rotationRad = 0;
            changed = true;
        }
        if (changed) {
            logParams();
            if (!maskChanged) pushPreview();
            else pushPreview();
            return true;
        }
        return false;
    },
};
