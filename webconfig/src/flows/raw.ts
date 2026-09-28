// The endpoints whose body or answer is bytes, not JSON: a piece of an
// upload, a piece of a download. Called with fetch directly, since the
// generated client wants JSON; the rules are the same (docs/api.md): an
// octet-stream body, the answer's refusal an ApiError.

import { ApiFailure, failure, HEADER_ACTIVITY, type Schemas } from "../api/client";

/** protocol::api::HEADER_SIZE and HEADER_MTIME. */
const HEADER_SIZE = "x-tessaro-size";
const HEADER_MTIME = "x-tessaro-mtime";

function target(path: string, query: Record<string, string | number>): string {
  const search = new URLSearchParams(Object.entries(query).map(([key, value]) => [key, String(value)]));
  return `${path}?${search.toString()}`;
}

async function refusal(response: Response): Promise<ApiFailure> {
  try {
    return failure(await response.json(), response.status);
  } catch {
    return new ApiFailure(`${response.status} ${response.statusText}`, "internal", response.status);
  }
}

/** One piece of an upload; the device answers how much it has now. */
export async function putPiece(
  path: "/api/v1/files/upload" | "/api/v1/update/image",
  query: Record<string, string | number>,
  piece: Blob,
  signal: AbortSignal,
): Promise<Schemas["Received"]> {
  let response: Response;
  try {
    response = await fetch(target(path, query), {
      method: "PUT",
      credentials: "same-origin",
      // A transfer someone started is use of the session, all of it.
      headers: { "content-type": "application/octet-stream", [HEADER_ACTIVITY]: "1" },
      body: piece,
      signal,
    });
  } catch (error) {
    throw failure(error);
  }
  if (!response.ok) {
    throw await refusal(response);
  }
  return (await response.json()) as Schemas["Received"];
}

export interface Piece {
  bytes: Uint8Array;
  /** The whole file's size and mtime, as the device says them now. */
  size: number;
  mtime: number;
}

/** Bytes of a stored file from `offset` on; empty at its end. */
export async function readPiece(path: string, offset: number, len: number, signal: AbortSignal): Promise<Piece> {
  let response: Response;
  try {
    response = await fetch(target("/api/v1/files/content", { path, offset, len }), {
      credentials: "same-origin",
      headers: { [HEADER_ACTIVITY]: "1" },
      signal,
    });
  } catch (error) {
    throw failure(error);
  }
  if (!response.ok) {
    throw await refusal(response);
  }
  const number = (name: string) => {
    const value = Number(response.headers.get(name));
    if (response.headers.get(name) === null || !Number.isFinite(value)) {
      throw new Error(`the device's answer has no ${name}`);
    }
    return value;
  };
  return {
    bytes: new Uint8Array(await response.arrayBuffer()),
    size: number(HEADER_SIZE),
    mtime: number(HEADER_MTIME),
  };
}
