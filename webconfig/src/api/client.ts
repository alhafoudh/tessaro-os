// The device's API, typed from agent/protocol/openapi.json (`npm run gen`
// writes schema.d.ts). Everything Webconfig asks goes through `client` or
// `$api`, so a path or a body the device does not have is a type error.

import createClient, { type Middleware } from "openapi-fetch";
import createQueryClient from "openapi-react-query";

import type { components, paths } from "./schema";
import { recentlyActive } from "./activity";

export type Schemas = components["schemas"];

/** protocol::api::HEADER_ACTIVITY: this request is someone's doing. */
export const HEADER_ACTIVITY = "x-tessaro-activity";

/** protocol::UPDATE_CHUNK: the largest piece of an upload or a download. */
export const CHUNK = 4 * 1024 * 1024;

/**
 * Only what someone did keeps a browser session alive: every write, and a
 * read made right after they touched the page. A page left open refreshes
 * without the header, and the session times out as if it were closed.
 */
const activity: Middleware = {
  onRequest({ request }) {
    if (request.method !== "GET" || recentlyActive()) {
      request.headers.set(HEADER_ACTIVITY, "1");
    }
    return request;
  },
};

export const client = createClient<paths>({ baseUrl: "", credentials: "same-origin" });
client.use(activity);

export const $api = createQueryClient(client);

/** A refusal or a failure, with the device's words when it answered. */
export class ApiFailure extends Error {
  readonly code: Schemas["ErrorCode"] | "lost";
  readonly status: number;

  constructor(message: string, code: Schemas["ErrorCode"] | "lost", status: number) {
    super(message);
    this.code = code;
    this.status = status;
  }

  /** No answer came: the device is restarting, or the link went away. */
  get lost(): boolean {
    return this.code === "lost";
  }

  /** The device wants a credential this browser does not have (any more). */
  get signedOut(): boolean {
    return this.code === "token-required" || this.code === "invalid-token";
  }
}

/** What went wrong, in words: the device's `error`, or the network's. */
export function failure(error: unknown, status = 0): ApiFailure {
  if (error instanceof ApiFailure) {
    return error;
  }
  if (error && typeof error === "object" && "error" in error && "code" in error) {
    const refusal = error as Schemas["ApiError"];
    return new ApiFailure(refusal.error, refusal.code, status);
  }
  if (error instanceof Error) {
    return new ApiFailure(`no answer: ${error.message}`, "lost", status);
  }
  return new ApiFailure(String(error), "lost", status);
}

/** The data of an openapi-fetch result, or its refusal thrown. */
export async function answer<T>(pending: Promise<{ data?: T; error?: unknown; response: Response }>): Promise<T> {
  return (await answered(pending)).data;
}

/** `answer`, with the response too, for an answer's headers. */
export async function answered<T>(
  pending: Promise<{ data?: T; error?: unknown; response: Response }>,
): Promise<{ data: T; response: Response }> {
  let result;
  try {
    result = await pending;
  } catch (error) {
    throw failure(error);
  }
  if (result.error !== undefined || !result.response.ok) {
    throw failure(result.error ?? `${result.response.status} ${result.response.statusText}`, result.response.status);
  }
  return { data: result.data as T, response: result.response };
}
