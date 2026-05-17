/**
 * Persistent "this file failed to load" banner state. Backs
 * `ui/LoadErrorToast.svelte` — separate from the auto-dismissing
 * `Toast` system because load errors warrant a stickier UI: the user
 * needs time to read the missing-features list and decide whether to
 * update.
 *
 * Payload shape matches `LoadError::to_json()` in the Rust core. Only
 * one banner shows at a time — a second `show()` replaces the first.
 */

/** Mirror of `crates/darkly/src/format/error.rs::LoadError::to_json()`. */
export type LoadErrorPayload =
    | {
          kind: 'containerTooNew';
          found: number;
          supported: number;
          message: string;
      }
    | {
          kind: 'unsupportedFeatures';
          missing: string[];
          message: string;
      }
    | {
          kind: 'corruptManifest';
          reason: string;
          message: string;
      }
    | {
          kind: 'unknownTypeId';
          registry: string;
          id: string;
          message: string;
      }
    | {
          kind: 'io' | 'zip' | 'json';
          message: string;
      }
    // Catch-all for any future variant the UI doesn't know about yet —
    // the toast falls back to `message` rather than crashing.
    | {
          kind: string;
          message?: string;
      };

class LoadErrorState {
    payload = $state<LoadErrorPayload | null>(null);

    show(payload: LoadErrorPayload) {
        this.payload = payload;
    }

    dismiss() {
        this.payload = null;
    }
}

export const loadError = new LoadErrorState();

/** Best-effort parse of the JSON payload that
 *  `DarklyHandle.open_document(bytes)` throws on refusal. Falls back
 *  to a generic message when the throwable isn't structured. */
export function parseLoadErrorMessage(e: unknown): LoadErrorPayload {
    const raw = e instanceof Error ? e.message : String(e);
    try {
        const parsed = JSON.parse(raw) as LoadErrorPayload;
        if (parsed && typeof parsed === 'object' && 'kind' in parsed) {
            return parsed;
        }
    } catch {
        // Not structured — fall through.
    }
    return { kind: 'json', message: raw };
}
