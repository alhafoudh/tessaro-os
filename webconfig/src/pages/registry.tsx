// The left menu: Quick Setup first, then the GUI's pages in the GUI's order
// (device.rs, `Page::TOOLS`), each with the settings its Configure opens
// (`Page::scope`). The setting groups no page claims follow, as sections of
// their own.

import type { ComponentType } from "react";

import type { Scope } from "../settings/scope";
import { Access } from "./Access";
import { Audio } from "./Audio";
import { Browser } from "./Browser";
import { Certificates } from "./Certificates";
import { Files } from "./Files";
import { Log } from "./Log";
import { Network } from "./Network";
import { Overview } from "./Overview";
import { Policies } from "./Policies";
import { QuickSetup } from "./QuickSetup";
import { Schedules } from "./Schedules";
import { Screen } from "./Screen";
import { Ssh } from "./Ssh";
import { Storage } from "./Storage";
import { Time } from "./Time";
import { Update } from "./Update";
import { Wifi } from "./Wifi";

export interface PageInfo {
  path: string;
  title: string;
  scope?: Scope;
  component: ComponentType<{ info: PageInfo }>;
}

export const PAGES: PageInfo[] = [
  { path: "quick-setup", title: "Quick Setup", component: QuickSetup },
  { path: "overview", title: "Overview", scope: { prefix: "device" }, component: Overview },
  { path: "screen", title: "Screen", scope: { prefix: "screen" }, component: Screen },
  { path: "browser", title: "Browser", scope: { prefix: "browser" }, component: Browser },
  { path: "policies", title: "Policies", component: Policies },
  { path: "network", title: "Network", scope: { prefix: "network", except: "network.wifi" }, component: Network },
  { path: "wifi", title: "WiFi", scope: { prefix: "network.wifi" }, component: Wifi },
  { path: "certificates", title: "Certificates", component: Certificates },
  { path: "storage", title: "Storage", scope: { prefix: "storage" }, component: Storage },
  { path: "audio", title: "Audio", scope: { prefix: "audio" }, component: Audio },
  { path: "time", title: "Time", scope: { prefix: "time" }, component: Time },
  { path: "schedules", title: "Schedules", component: Schedules },
  { path: "access", title: "Access", scope: { prefix: "access" }, component: Access },
  { path: "ssh", title: "SSH", component: Ssh },
  { path: "files", title: "Files", component: Files },
  { path: "update", title: "Update", component: Update },
  { path: "log", title: "Log", component: Log },
];

/** The scopes the pages claim, which the own sections leave out. */
export const CLAIMED: Scope[] = PAGES.flatMap((page) => (page.scope ? [page.scope] : []));

/** A section's title: `data` is Data, `agent` Agent. */
export function sectionTitle(section: string): string {
  return section.charAt(0).toUpperCase() + section.slice(1);
}
