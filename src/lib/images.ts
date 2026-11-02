// Where the interface loads an image from. The web view asks for a page's
// plain-http images over https instead (as browsers now do for secure pages),
// and many providers' image servers speak only http, so those go through the
// app, which fetches them as given (xorvio-core's images module, whose
// image_url makes the same addresses).

import { isTauri } from "./api";

const windows = typeof navigator !== "undefined" && /Windows/.test(navigator.userAgent);
const BASE = windows ? "http://xorvio-image.localhost/" : "xorvio-image://localhost/";

export function imageSrc(src: string): string;
export function imageSrc(src: string | null | undefined): string | undefined;
export function imageSrc(src: string | null | undefined): string | undefined {
  if (!src) return undefined;
  return isTauri && /^http:\/\//i.test(src) ? BASE + encodeURIComponent(src) : src;
}
