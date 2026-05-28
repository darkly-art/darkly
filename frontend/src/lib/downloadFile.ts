/** Trigger a browser download of `bytes` as `filename` via an anchor click.
 *  Pulled out into its own module so tests can mock it via `vi.mock` without
 *  having to stand up a jsdom environment. */
export function downloadFile(bytes: BlobPart, filename: string, mime = 'application/octet-stream'): void {
    const blob = new Blob([bytes], { type: mime });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    a.click();
    URL.revokeObjectURL(url);
}
