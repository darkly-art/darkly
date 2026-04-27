/**
 * State for the Krita brush inspector page (`?brush-inspect`).
 *
 * Holds one parsed preset at a time, plus the raw `KritaInspector` WASM
 * handle so resource panels can pull bytes lazily for `URL.createObjectURL`.
 */
import init, { KritaInspector } from '../../wasm/pkg/darkly_wasm';

// --------------------------------------------------------------------------
// JSON shapes — mirror crates/darkly/src/brush/import/krita/.
// Kept in this file so the inspector is self-contained. If the Rust types
// gain fields, add them here too.
// --------------------------------------------------------------------------

export type ParamDecoded =
    | { kind: 'plain'; value: string }
    | { kind: 'curve'; points: [number, number][] }
    | { kind: 'sensor_xml'; sensor_id: string | null; xml: string }
    | { kind: 'bytearray'; byte_length: number }
    | { kind: 'nested_xml'; xml: string };

export interface KritaParam {
    name: string;
    raw_type: string | null;
    raw_value: string;
    decoded: ParamDecoded;
}

export type ResourceFormat =
    | { kind: 'png'; width: number | null; height: number | null }
    | { kind: 'jpeg' }
    | { kind: 'svg' }
    | { kind: 'gbr' }
    | { kind: 'gih' }
    | { kind: 'abr' }
    | { kind: 'unknown'; magic_hex: string };

export interface KritaResource {
    name: string;
    filename: string;
    resource_type: string;
    md5sum: string;
    byte_length: number;
    format: ResourceFormat;
}

export interface PngChunkInfo {
    chunk_type: string;
    byte_length: number;
    text_keyword: string | null;
    text_length: number | null;
}

export interface PngInfo {
    width: number;
    height: number;
    color_type: string;
    bit_depth: number;
    chunks: PngChunkInfo[];
}

export interface KritaPreset {
    format_version: string;
    paintop_id: string;
    paintop_description: string | null;
    preset_name: string | null;
    embedded_resources_attr: number | null;
    png: PngInfo;
    params: KritaParam[];
    resources: KritaResource[];
    preset_xml_elided: string;
}

// --------------------------------------------------------------------------
// Reactive state
// --------------------------------------------------------------------------

interface LoadedFile {
    name: string;
    byteLength: number;
    preset: KritaPreset;
    handle: KritaInspector;
}

class InspectorState {
    file = $state<LoadedFile | null>(null);
    error = $state<string | null>(null);
    loading = $state(false);
    private wasmReady: Promise<void> | null = null;

    async load(file: File): Promise<void> {
        this.error = null;
        this.loading = true;
        try {
            if (!this.wasmReady) {
                this.wasmReady = init().then(() => undefined);
            }
            await this.wasmReady;
            const buf = await file.arrayBuffer();
            const bytes = new Uint8Array(buf);
            // Free any previous handle before swapping.
            this.file?.handle.free();
            const handle = new KritaInspector(bytes);
            const preset = JSON.parse(handle.metadata()) as KritaPreset;
            this.file = {
                name: file.name,
                byteLength: bytes.byteLength,
                preset,
                handle,
            };
        } catch (e) {
            this.error = e instanceof Error ? e.message : String(e);
            this.file?.handle.free();
            this.file = null;
        } finally {
            this.loading = false;
        }
    }

    clear(): void {
        this.file?.handle.free();
        this.file = null;
        this.error = null;
    }

    /** Get a Blob URL for an embedded resource. Caller is responsible for
     *  revoking the URL when the element unmounts. */
    resourceBlobUrl(index: number): string | null {
        if (!this.file) return null;
        const bytes = this.file.handle.resource_bytes(index);
        const fmt = this.file.preset.resources[index]?.format.kind;
        const mime = mimeForFormat(fmt);
        const blob = new Blob([bytes], { type: mime });
        return URL.createObjectURL(blob);
    }
}

function mimeForFormat(kind: string | undefined): string {
    switch (kind) {
        case 'png':
            return 'image/png';
        case 'jpeg':
            return 'image/jpeg';
        case 'svg':
            return 'image/svg+xml';
        default:
            return 'application/octet-stream';
    }
}

export const inspector = new InspectorState();
