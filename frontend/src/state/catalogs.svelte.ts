/**
 * The Rust core's registries, projected for the UI.
 *
 * Every registry (effects, tools, voids, blend modes, actions, ...) is built in
 * Rust from `&'static` registrations with no engine or document state: the
 * handler at `crates/darkly/src/engine/catalogs.rs` just delegates to the
 * GPU-free free function `crate::catalog::catalogs()`. So every handle answers
 * `catalogs` with byte-identical data, and this is process state, not per-canvas
 * state: module level rather than per `DarklyInstance`, for the same reason
 * `state/brushColors.svelte.ts` and `state/recents.svelte.ts` are.
 *
 * Per-instance payloads (`LayerInfo`, ...) carry only the stable `type_id`; the
 * UI resolves the human-readable label and the icon through here, so neither
 * exists in a second copy.
 */

import type { EngineRequests } from '../engine/protocol';
import type { Catalog, CatalogEntry } from '../engine/protocol_gen';
import type { CaptureKind } from '../lib/frameSource';
import { toolRegistry } from '../tools/registry';
import { tooltipForAction } from '../config/store.svelte';

export class Catalogs {
    /** Every registry the core declares, keyed by catalog id. Empty until
     *  {@link load} resolves. */
    #byId = $state<Record<string, Catalog>>({});

    /** The in-flight (or settled) load. Shared, so concurrent callers get the
     *  one request rather than an empty catalog; see {@link load}. */
    #loading: Promise<void> | null = null;

    /**
     * Fetch the registries through `engine`, once per process.
     *
     * Returns the shared in-flight promise when a load is already running. That
     * matters rather than being a nicety: crash recovery opens one tab per
     * snapshot (`state/recovery.svelte.ts`), each bootstrapping concurrently,
     * and each feeds `actions.setDocs` from the `actions` catalog right after
     * awaiting this. A plain "already loading" guard would return early to the
     * second caller while the first request was still out, handing it an empty
     * catalog and losing every action's documentation for the session.
     *
     * Any handle can serve the load: the answer does not depend on which.
     * Takes the request surface rather than a concrete `Engine` (the rule
     * `engine/protocol.ts` states for helpers that only issue requests), which
     * is also what makes it testable without a canvas.
     */
    load(engine: EngineRequests): Promise<void> {
        this.#loading ??= (async () => {
            const byId: Record<string, Catalog> = {};
            for (const c of (await engine.api.catalogs()) ?? []) byId[c.id] = c;
            this.#byId = byId;
        })();
        return this.#loading;
    }

    /** One catalog by id, or `undefined` when it is unknown. */
    catalog(catalogId: string): Catalog | undefined {
        return this.#byId[catalogId];
    }

    /** Entries of one catalog, or an empty array when it is unknown. */
    entries(catalogId: string): CatalogEntry[] {
        return this.#byId[catalogId]?.entries ?? [];
    }

    /** One entry by catalog and `type_id`, or `undefined`. */
    entry(catalogId: string, typeId: string): CatalogEntry | undefined {
        return this.entries(catalogId).find((e) => e.type === typeId);
    }

    /** Display label for a `type_id` within a catalog (e.g. `"curves"` to
     *  `"Curves"`), falling back to the id itself when unknown. */
    displayName(catalogId: string, typeId: string): string {
        return this.entry(catalogId, typeId)?.displayName ?? typeId;
    }

    /** The Iconify glyph to render for a tool.
     *
     *  A tool's glyph is registry metadata and lives on its Rust registration.
     *  A descriptor may override it when the glyph depends on live session
     *  state a static registration cannot express: the brush shows the eraser
     *  icon while erase mode is on. This is the single place that precedence
     *  is decided, so no caller branches on a tool id. */
    toolGlyph(typeId: string): string {
        const override = toolRegistry.get(typeId)?.icon;
        const resolved = typeof override === 'function' ? override() : override;
        return resolved ?? this.entry('tools', typeId)?.icon ?? 'fa6-solid:wrench';
    }

    /** A tool button's `title`: its label plus the chord currently bound to
     *  the action that selects it. Both the label and that action id are the
     *  tool's own registry metadata, so resolving them together here keeps the
     *  toolbar and the cluster flyout from each doing the lookup. */
    toolTooltip(typeId: string): string {
        const entry = this.entry('tools', typeId);
        return tooltipForAction(entry?.displayName ?? typeId, entry?.hotkeyAction ?? '');
    }

    /** `voidType` to `CaptureKind` for voids backed by a browser MediaStream
     *  (camera / screenshare); procedural and image-sourced voids are absent.
     *  Drives which `MediaDevices` API to call and is the single source of
     *  truth for "is this a stream-backed void?" across the reconciler, picker,
     *  and properties panel. */
    voidCaptureKind = $derived.by(() => {
        const kinds = new Map<string, CaptureKind>();
        for (const v of this.entries('voids')) {
            if (v.source?.kind === 'capture') kinds.set(v.type, v.source.capture);
        }
        return kinds;
    });

}

/** The process-wide registries. Tests that need an unloaded one construct
 *  their own `new Catalogs()`. */
export const catalogs = new Catalogs();
