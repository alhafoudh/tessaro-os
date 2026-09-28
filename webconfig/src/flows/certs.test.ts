import { describe, expect, it } from "vitest";

import { readPem } from "./certs";

describe("certificate files, as client/src/certs.rs reads them", () => {
  it("sends PEM as it is and wraps DER", () => {
    const text = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n";
    expect(readPem("ca.pem", new TextEncoder().encode(text))).toEqual({ pem: text });
    expect(readPem("ca.cer", new Uint8Array([0x30, 0x82, 0xff, 0x00]))).toEqual({
      pem: "-----BEGIN CERTIFICATE-----\nMIL/AA==\n-----END CERTIFICATE-----\n",
    });
  });

  it("keeps keys and other files here", () => {
    const key =
      "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n-----BEGIN PRIVATE KEY-----\nMIIE\n-----END PRIVATE KEY-----\n";
    expect(readPem("key.pem", new TextEncoder().encode(key))).toEqual({
      error: "key.pem: holds a private key; send only the certificate",
    });
    expect(readPem("notes.txt", new TextEncoder().encode("hello"))).toEqual({
      error: "notes.txt: not a certificate (neither PEM nor DER)",
    });
  });
});
