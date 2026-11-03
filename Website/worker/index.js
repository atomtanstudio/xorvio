// Serves the static site. Sends www.xorvio.com to xorvio.com, serves the large media
// (the launch film) from R2, and adds the security and caching headers every response
// should carry.
const SECURITY = {
  "Strict-Transport-Security": "max-age=31536000",
  "X-Content-Type-Options": "nosniff",
  "Referrer-Policy": "strict-origin-when-cross-origin",
  "Permissions-Policy": "camera=(), microphone=(), geolocation=(), interest-cohort=()",
  "Cross-Origin-Opener-Policy": "same-origin",
  "Content-Security-Policy":
    "default-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; font-src 'self'; " +
    "connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'; upgrade-insecure-requests",
};

// Files too big for the static assets (25 MB each, and no byte ranges, which Safari
// needs before it plays a video at all) live in the xorvio-media R2 bucket and are
// served at /media/<key>. The keys carry a content hash, so the files are cached for
// a year; src/media.json names them. Returns null when there's no such object, so the
// site's own 404 page answers.
async function media(request, env, key) {
  if (request.method !== "GET" && request.method !== "HEAD") {
    return new Response("Method not allowed", { status: 405, headers: { Allow: "GET, HEAD" } });
  }
  let object;
  try {
    object = request.method === "HEAD"
      ? await env.MEDIA.head(key)
      : await env.MEDIA.get(key, { range: request.headers, onlyIf: request.headers });
  } catch {
    return new Response("Range not satisfiable", { status: 416 });
  }
  if (!object) return null;
  const headers = new Headers();
  object.writeHttpMetadata(headers);
  headers.set("ETag", object.httpEtag);
  headers.set("Accept-Ranges", "bytes");
  headers.set("Cache-Control", "public, max-age=31536000, immutable");
  if (request.method === "HEAD") {
    headers.set("Content-Length", String(object.size));
    return new Response(null, { status: 200, headers });
  }
  // A matching If-None-Match gives an object without a body.
  if (!("body" in object)) return new Response(null, { status: 304, headers });
  let status = 200;
  let length = object.size;
  // R2 parsed the Range header into offset and length, or a suffix (the last n bytes).
  // Check the fields, not the keys: the local R2 hands back an object with undefined
  // fields when nothing was asked for.
  const range = request.headers.has("Range") ? object.range : undefined;
  if (range && (range.suffix != null || range.offset != null || range.length != null)) {
    let start, end;
    if (range.suffix != null) {
      start = Math.max(0, object.size - range.suffix);
      end = object.size - 1;
    } else {
      start = range.offset || 0;
      end = range.length != null ? Math.min(start + range.length, object.size) - 1 : object.size - 1;
    }
    status = 206;
    length = end - start + 1;
    headers.set("Content-Range", `bytes ${start}-${end}/${object.size}`);
  }
  headers.set("Content-Length", String(length));
  return new Response(object.body, { status, headers });
}

function mediaKey(pathname) {
  try {
    return decodeURIComponent(pathname.slice("/media/".length));
  } catch {
    return "";
  }
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.hostname.startsWith("www.")) {
      url.hostname = url.hostname.slice(4);
      return Response.redirect(url.toString(), 301);
    }
    const fromMedia = url.pathname.startsWith("/media/") ? await media(request, env, mediaKey(url.pathname)) : null;
    const response = fromMedia || (await env.ASSETS.fetch(request));
    const headers = new Headers(response.headers);
    for (const [name, value] of Object.entries(SECURITY)) headers.set(name, value);
    const type = headers.get("Content-Type") || "";
    if (/^\/(img|fonts|media)\//.test(url.pathname)) {
      headers.set("Cache-Control", "public, max-age=31536000, immutable");
    } else if (/\.(css|js)$/.test(url.pathname)) {
      headers.set("Cache-Control", "public, max-age=31536000, immutable");
    } else if (type.includes("text/html")) {
      headers.set("Cache-Control", "public, max-age=0, must-revalidate");
    }
    return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
  },
};
