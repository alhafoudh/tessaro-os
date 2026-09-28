// What `mise run webconfig:run` changes between the browser and a device,
// so the dev server on http://localhost can stand in for the device's own
// origin (docs/webconfig.md, "Developing").
//
// The device refuses a browser write from another origin, and its cookie is
// `__Host-` and `Secure`, which a browser keeps only for https. So the proxy
// says the request came from the device's origin, and hands the browser a
// plain cookie under another name, mapped back on the way in.

const DEVICE_PREFIX = "__Host-tessaro-";
const DEV_PREFIX = "tessaro-dev-";

/** A `Set-Cookie` from the device, as the dev server's browser can keep it. */
export function cookieToBrowser(header: string): string {
  const [pair = "", ...attributes] = header.split(";").map((part) => part.trim());
  const name = pair.startsWith(DEVICE_PREFIX) ? DEV_PREFIX + pair.slice(DEVICE_PREFIX.length) : pair;
  const kept = attributes.filter((attribute) => attribute.toLowerCase() !== "secure");
  return [name, ...kept].join("; ");
}

/** The browser's `Cookie` header, with the device's cookie names back. */
export function cookieToDevice(header: string): string {
  return header
    .split(";")
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
    .map((part) => (part.startsWith(DEV_PREFIX) ? DEVICE_PREFIX + part.slice(DEV_PREFIX.length) : part))
    .join("; ");
}

/** `https://host:port` of the device the dev server talks to. */
export function originOf(target: string): string {
  return new URL(target).origin;
}
