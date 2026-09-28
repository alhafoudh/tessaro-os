import { describe, expect, it } from "vitest";

import { cookieToBrowser, cookieToDevice, originOf } from "./dev-proxy";

describe("the dev server's proxy", () => {
  it("gives the browser a plain cookie under a name of its own", () => {
    expect(cookieToBrowser("__Host-tessaro-abcd=f00; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=60")).toBe(
      "tessaro-dev-abcd=f00; Path=/; HttpOnly; SameSite=Strict; Max-Age=60",
    );
  });

  it("maps the name back for the device and leaves other cookies alone", () => {
    expect(cookieToDevice("a=b; tessaro-dev-abcd=f00")).toBe("a=b; __Host-tessaro-abcd=f00");
  });

  it("says the request came from the device's origin", () => {
    expect(originOf("https://127.0.0.1:17400/")).toBe("https://127.0.0.1:17400");
  });
});
