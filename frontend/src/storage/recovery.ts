/**
 * Crash-recovery snapshot store. Autosave writes each unsaved document as
 * an ordinary `.darkly` zip into OPFS under `recovery/`; on a detected
 * crash the startup flow restores them via the engine's `open_document`.
 *
 * There is deliberately **no separate metadata sidecar or index file**:
 * the snapshot filename encodes both the owning browser session and the
 * tab's stable recovery id, and the human-facing document name is read
 * straight from the zip's `manifest.json`. A single write per snapshot
 * means there is nothing to keep in sync and nothing to leave torn: the
 * exact failure this feature exists to survive. A snapshot that fails to
 * unzip / open is simply skipped by the caller (offered for discard).
 *
 * Crash attribution (which session a snapshot belongs to) lives in the
 * filename; the live-vs-crashed decision lives in `recoverySession.ts`.
 */
import { unzipSync } from 'fflate';
import { storage as defaultStorage, type DarklyStorage } from './index';

const RECOVERY_DIR = 'recovery';
/** Filename separator between session id and recovery id. Neither id (a
 *  UUID or `crypto`-less base36 fallback) contains `~`, so parsing back is
 *  unambiguous. */
const SEP = '~';
const SUFFIX = '.darkly';
const MANIFEST_ENTRY = 'manifest.json';
const THUMBNAIL_ENTRY = 'thumbnail.png';

const textDecoder = new TextDecoder();

/** A recoverable snapshot on disk, identified by its owning session and
 *  the tab's stable recovery id. */
export interface RecoveryEntry {
    sessionId: string;
    recoveryId: string;
    /** Document display name, read from the zip's manifest. */
    name: string;
}

function snapshotPath(sessionId: string, recoveryId: string): string {
    return `${RECOVERY_DIR}/${sessionId}${SEP}${recoveryId}${SUFFIX}`;
}

/** Parse a `recovery/` filename back into its ids, or null if it isn't a
 *  well-formed snapshot name. */
function parseSnapshotName(filename: string): { sessionId: string; recoveryId: string } | null {
    if (!filename.endsWith(SUFFIX)) return null;
    const stem = filename.slice(0, -SUFFIX.length);
    const sep = stem.indexOf(SEP);
    if (sep <= 0 || sep >= stem.length - 1) return null;
    return { sessionId: stem.slice(0, sep), recoveryId: stem.slice(sep + 1) };
}

/** Extract a single entry's bytes from a `.darkly` zip, or null if the zip
 *  is corrupt or the entry is missing. Pure (fflate), node-safe for tests. */
export function extractZipEntry(zipBytes: Uint8Array, entry: string): Uint8Array | null {
    try {
        const files = unzipSync(zipBytes, { filter: (f) => f.name === entry });
        return files[entry] ?? null;
    } catch {
        return null;
    }
}

/** Read the document display name out of a snapshot zip's manifest.
 *  Falls back to "Recovered document" when the manifest is unreadable. */
export function snapshotDocName(zipBytes: Uint8Array): string {
    const manifest = extractZipEntry(zipBytes, MANIFEST_ENTRY);
    if (!manifest) return 'Recovered document';
    try {
        const parsed = JSON.parse(textDecoder.decode(manifest)) as { name?: string };
        return parsed.name?.trim() || 'Recovered document';
    } catch {
        return 'Recovered document';
    }
}

/** The PNG thumbnail bytes from a snapshot zip, or null if absent. */
export function snapshotThumbnail(zipBytes: Uint8Array): Uint8Array | null {
    return extractZipEntry(zipBytes, THUMBNAIL_ENTRY);
}

/**
 * Write (or overwrite) a tab's recovery snapshot. A single atomic-enough
 * write per call: the snapshot is keyed by `(sessionId, recoveryId)`, so
 * repeated autosaves of the same tab replace one file rather than piling
 * up. A crash mid-write can only ever damage *this* tab's latest snapshot,
 * never the index of others (there is none).
 */
export async function writeSnapshot(
    sessionId: string,
    recoveryId: string,
    zipBytes: Uint8Array,
    storage: DarklyStorage = defaultStorage,
): Promise<void> {
    await storage.write(snapshotPath(sessionId, recoveryId), zipBytes);
}

/** Raw snapshot bytes, or null if the snapshot is missing. */
export async function readSnapshot(
    sessionId: string,
    recoveryId: string,
    storage: DarklyStorage = defaultStorage,
): Promise<Uint8Array | null> {
    return storage.read(snapshotPath(sessionId, recoveryId));
}

/** Delete a tab's snapshot. Idempotent. */
export async function removeSnapshot(
    sessionId: string,
    recoveryId: string,
    storage: DarklyStorage = defaultStorage,
): Promise<void> {
    await storage.remove(snapshotPath(sessionId, recoveryId));
}

/**
 * Every snapshot currently on disk, with its document name resolved from
 * the zip. Corrupt / unreadable snapshots are dropped (a snapshot we can't
 * open isn't recoverable). One-time startup scan: reading each zip to
 * pull the manifest name is cheap at recovery scale.
 */
export async function listSnapshots(
    storage: DarklyStorage = defaultStorage,
): Promise<RecoveryEntry[]> {
    const dir = await storage.list(RECOVERY_DIR);
    const out: RecoveryEntry[] = [];
    for (const entry of dir) {
        if (entry.kind !== 'file') continue;
        const ids = parseSnapshotName(entry.name);
        if (!ids) continue;
        const bytes = await storage.read(`${RECOVERY_DIR}/${entry.name}`);
        if (!bytes || !extractZipEntry(bytes, MANIFEST_ENTRY)) continue; // corrupt → skip
        out.push({ ...ids, name: snapshotDocName(bytes) });
    }
    return out;
}
