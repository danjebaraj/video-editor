/**
 * Fills in the details of the newest Mac release wherever a page marks them:
 *   [data-release="version"]  "0.1 (1)"
 *   [data-release="size"]     "48 MB"
 *   [data-release="date"]     "November 18, 2026"
 *   [data-release="sha256"]   the checksum
 *   [data-release-if="unnotarized"]  shown only when the build isn't notarized
 *   [data-release-if="missing"]      shown only when nothing is published yet
 * The details come from the Worker (/releases/latest/mac.json), so a new upload shows up
 * without rebuilding the site.
 */
export interface Release {
  version: string;
  build: string;
  file: string;
  size: number;
  sha256: string;
  minimumSystemVersion: string;
  architectures: string;
  notarized: boolean;
  published: string;
}

let pending: Promise<Release | null> | null = null;
export function fetchRelease(): Promise<Release | null> {
  pending ??= fetch("/releases/latest/mac.json", { headers: { accept: "application/json" } })
    .then((r) => (r.ok ? (r.json() as Promise<Release>) : null))
    .catch(() => null);
  return pending;
}

function megabytes(bytes: number): string {
  const mb = bytes / 1_048_576;
  return `${mb < 10 ? mb.toFixed(1) : Math.round(mb)} MB`;
}

export async function fillRelease(root: ParentNode = document): Promise<Release | null> {
  const release = await fetchRelease();
  const set = (name: string, value: string) =>
    root.querySelectorAll<HTMLElement>(`[data-release="${name}"]`).forEach((el) => (el.textContent = value));
  const show = (name: string, visible: boolean) =>
    root.querySelectorAll<HTMLElement>(`[data-release-if="${name}"]`).forEach((el) => (el.hidden = !visible));

  show("missing", release === null);
  if (!release) return null;
  set("version", `${release.version} (${release.build})`);
  set("size", megabytes(release.size));
  set("date", new Date(release.published).toLocaleDateString(undefined, { year: "numeric", month: "long", day: "numeric" }));
  set("sha256", release.sha256);
  set("macos", `macOS ${release.minimumSystemVersion.replace(/\.0$/, "")} or later`);
  show("unnotarized", !release.notarized);
  return release;
}
