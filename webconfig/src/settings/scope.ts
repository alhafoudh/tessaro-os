// Which settings a page's Configure shows, as the GUI's `Page::scope` and
// `Scope` (device.rs): the keys under a prefix, less those under another.

import type { Schemas } from "../api/client";
import { restartsShort } from "../describe/device";

export interface Scope {
  prefix: string;
  except?: string;
}

/** protocol::keys::DATA_PREFIX. */
export const DATA_PREFIX = "data.";
export const DATA_SECTION = "data";

function under(key: string, prefix: string): boolean {
  return key === prefix || key.startsWith(`${prefix}.`);
}

export function holds(scope: Scope, key: string): boolean {
  return under(key, scope.prefix) && !(scope.except && under(key, scope.except));
}

/** The settings' groups, in the order the device lists them. */
export function sections(settings: Schemas["Settings"]): string[] {
  const seen: string[] = [];
  for (const setting of settings.settings) {
    const group = setting.key.split(".")[0] ?? setting.key;
    if (!seen.includes(group)) seen.push(group);
  }
  return seen;
}

/** The groups no page claims, Data always among them. */
export function ownSections(settings: Schemas["Settings"], claimed: Scope[]): string[] {
  const own = sections(settings).filter((section) => !claimed.some((scope) => scope.prefix === section));
  if (!own.includes(DATA_SECTION)) own.push(DATA_SECTION);
  return own;
}

/** What the registry says about `key`; a `data.*` key is its template's. */
export function info(keys: Map<string, Schemas["KeyInfo"]>, key: string): Schemas["KeyInfo"] | undefined {
  return keys.get(key) ?? (key.startsWith(DATA_PREFIX) ? keys.get(`${DATA_PREFIX}<name>`) : undefined);
}

export interface SettingRow {
  key: string;
  short: string;
  value: string;
  default: string;
  source: Schemas["Source"];
  applies: string;
  guarded: boolean;
  info: Schemas["KeyInfo"] | undefined;
}

export function rows(settings: Schemas["Settings"], keys: Map<string, Schemas["KeyInfo"]>, scope: Scope): SettingRow[] {
  return settings.settings
    .filter((setting) => holds(scope, setting.key))
    .map((setting) => {
      const known = info(keys, setting.key);
      const short = setting.key.startsWith(`${scope.prefix}.`)
        ? setting.key.slice(scope.prefix.length + 1)
        : setting.key;
      return {
        key: setting.key,
        short,
        value: setting.value ?? "",
        default: known?.default ?? "",
        source: setting.source,
        applies: (known?.applies ?? []).map(restartsShort).join(", "),
        guarded: known?.guarded ?? false,
        info: known,
      };
    });
}

/** A `data.*` name: lower-case letters, digits and `_`, up to 32. */
export function isParam(name: string): boolean {
  return /^[a-z0-9_]{1,32}$/.test(name);
}
