/**
 * The docs site's Worker. Everything is static (Workers Static Assets) except the downloads,
 * which come from the R2 bucket `video-studio-releases` (see scripts/publish-mac.sh):
 *
 *   /releases/latest/mac        302 → the newest Mac installer (named in mac/latest.json)
 *   /releases/latest/mac.json   that release's details: version, size, sha256, …
 *   /releases/mac/<file>        the installer itself, with range requests and conditional GETs
 *
 * The types are declared here so the site needs no extra packages.
 */

interface R2Range { offset?: number; length?: number; suffix?: number }
interface R2ObjectBody {
  key: string;
  size: number;
  httpEtag: string;
  range?: R2Range;
  body?: ReadableStream;
  writeHttpMetadata(headers: Headers): void;
  json<T>(): Promise<T>;
}
interface R2Bucket {
  get(key: string, options?: { range?: Headers; onlyIf?: Headers }): Promise<R2ObjectBody | null>;
  head(key: string): Promise<R2ObjectBody | null>;
}
interface Fetcher { fetch(request: Request): Promise<Response> }
interface Env { ASSETS: Fetcher; RELEASES: R2Bucket }

const PLATFORMS = new Set(["mac"]);

function notFound(message = "Not found"): Response {
  return new Response(message, { status: 404, headers: { "content-type": "text/plain; charset=utf-8" } });
}

async function latest(env: Env, platform: string): Promise<{ file: string } | null> {
  const obj = await env.RELEASES.get(`${platform}/latest.json`);
  return obj ? obj.json<{ file: string }>() : null;
}

async function serveFile(request: Request, env: Env, key: string): Promise<Response> {
  const head = request.method === "HEAD";
  const obj = head
    ? await env.RELEASES.head(key)
    : await env.RELEASES.get(key, { range: request.headers, onlyIf: request.headers });
  if (!obj) return notFound("That download doesn't exist.");

  const headers = new Headers();
  obj.writeHttpMetadata(headers);
  headers.set("etag", obj.httpEtag);
  headers.set("accept-ranges", "bytes");
  headers.set("content-disposition", `attachment; filename="${key.split("/").pop()}"`);
  headers.set("x-content-type-options", "nosniff");

  if (head) {
    headers.set("content-length", String(obj.size));
    return new Response(null, { headers });
  }
  // A conditional GET that matched: R2 returns the object without a body.
  if (!obj.body) return new Response(null, { status: 304, headers });

  const r = obj.range;
  if (r && request.headers.has("range")) {
    const start = r.suffix !== undefined ? obj.size - r.suffix : (r.offset ?? 0);
    const length = r.suffix !== undefined ? r.suffix : (r.length ?? obj.size - start);
    headers.set("content-range", `bytes ${start}-${start + length - 1}/${obj.size}`);
    headers.set("content-length", String(length));
    return new Response(obj.body, { status: 206, headers });
  }
  headers.set("content-length", String(obj.size));
  return new Response(obj.body, { headers });
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const parts = url.pathname.replace(/^\/releases\/?/, "").split("/").filter(Boolean);
    if (!url.pathname.startsWith("/releases")) return env.ASSETS.fetch(request);
    if (request.method !== "GET" && request.method !== "HEAD") {
      return new Response("Method not allowed", { status: 405, headers: { allow: "GET, HEAD" } });
    }

    // /releases/latest/mac and /releases/latest/mac.json
    if (parts[0] === "latest" && parts.length === 2) {
      const wantsJson = parts[1].endsWith(".json");
      const platform = parts[1].replace(/\.json$/, "");
      if (!PLATFORMS.has(platform)) return notFound();
      if (wantsJson) {
        const obj = await env.RELEASES.get(`${platform}/latest.json`);
        if (!obj) return notFound("No release has been published yet.");
        return new Response(obj.body, {
          headers: {
            "content-type": "application/json; charset=utf-8",
            "cache-control": "no-cache",
            "access-control-allow-origin": "*",
          },
        });
      }
      const release = await latest(env, platform);
      if (!release) return notFound("No release has been published yet.");
      return new Response(null, {
        status: 302,
        headers: { location: `/releases/${platform}/${encodeURIComponent(release.file)}`, "cache-control": "no-store" },
      });
    }

    // /releases/mac/<file>
    if (parts.length === 2 && PLATFORMS.has(parts[0]) && !parts[1].endsWith(".json")) {
      return serveFile(request, env, `${parts[0]}/${decodeURIComponent(parts[1])}`);
    }
    return notFound();
  },
};
