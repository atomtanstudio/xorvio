// Serves the static site. Sends www.xorvio.com to xorvio.com and adds the
// security and caching headers every response should carry.
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

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.hostname.startsWith("www.")) {
      url.hostname = url.hostname.slice(4);
      return Response.redirect(url.toString(), 301);
    }
    const response = await env.ASSETS.fetch(request);
    const headers = new Headers(response.headers);
    for (const [name, value] of Object.entries(SECURITY)) headers.set(name, value);
    const type = headers.get("Content-Type") || "";
    if (/^\/(img|fonts)\//.test(url.pathname)) {
      headers.set("Cache-Control", "public, max-age=31536000, immutable");
    } else if (/\.(css|js)$/.test(url.pathname)) {
      headers.set("Cache-Control", "public, max-age=31536000, immutable");
    } else if (type.includes("text/html")) {
      headers.set("Cache-Control", "public, max-age=0, must-revalidate");
    }
    return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
  },
};
