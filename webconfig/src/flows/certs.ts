// agent/client/src/certs.rs: a certificate file as the PEM text the
// protocol carries. PEM goes as it is; a binary DER certificate (a `.cer`
// or `.crt` as Windows exports them) is wrapped first. A private key is
// refused here, before it leaves this machine. The device checks the rest.

/** protocol::CERT_PEM_MAX. */
export const CERT_PEM_MAX = 64 * 1024;

function derToPem(der: Uint8Array): string {
  let binary = "";
  for (const byte of der) binary += String.fromCharCode(byte);
  const body = btoa(binary);
  const lines = body.match(/.{1,64}/g) ?? [];
  return `-----BEGIN CERTIFICATE-----\n${lines.join("\n")}\n-----END CERTIFICATE-----\n`;
}

/** The certificates in `bytes` (a file named `name`), as PEM, or why not. */
export function readPem(name: string, bytes: Uint8Array): { pem: string } | { error: string } {
  if (bytes.length > CERT_PEM_MAX) {
    return { error: `${name}: ${bytes.length} bytes; a certificate file is at most ${CERT_PEM_MAX}` };
  }
  let text: string | null;
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    text = null;
  }
  let pem: string;
  if (text !== null && text.includes("-----BEGIN")) {
    pem = text;
  } else if (bytes[0] === 0x30) {
    // DER: a SEQUENCE, so its first byte is 0x30.
    pem = derToPem(bytes);
  } else {
    return { error: `${name}: not a certificate (neither PEM nor DER)` };
  }
  return checkPem(name, pem);
}

/** Pasted text, held to the same rules as a file. */
export function checkPem(name: string, pem: string): { pem: string } | { error: string } {
  if (pem.includes("PRIVATE KEY-----")) {
    return { error: `${name}: holds a private key; send only the certificate` };
  }
  if (!pem.includes("-----BEGIN CERTIFICATE-----")) {
    return { error: `${name}: no certificate in it` };
  }
  return { pem };
}
