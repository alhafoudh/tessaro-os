// The demo's sections, in the order of the home page. A new feature of the
// device gets its section here: an entry, its detect() in detect.ts, and
// its page in sections/ (docs/demo.md, "Adding a section").

import type { ComponentType } from "react";

import { AudioSection } from "../sections/Audio";
import { BrowserSection } from "../sections/Browser";
import { CameraSection } from "../sections/Camera";
import { DataSection } from "../sections/Data";
import { DeviceSection } from "../sections/Device";
import { FilesSection } from "../sections/Files";
import { GraphicsSection } from "../sections/Graphics";
import { KeyboardSection } from "../sections/Keyboard";
import { NetworkSection } from "../sections/Network";
import { PeripheralsSection } from "../sections/Peripherals";
import { PlaylistSection } from "../sections/Playlist";
import { PresenceSection } from "../sections/Presence";
import { PrintingSection } from "../sections/Printing";
import { RemoteSection } from "../sections/Remote";
import { ScriptsSection } from "../sections/Scripts";
import { TouchSection } from "../sections/Touch";
import { VideoSection } from "../sections/Video";
import type { IconName } from "../shell/icons";
import * as detect from "./detect";
import type { FeatureStatus } from "./status";

export interface SectionProps {
  status: FeatureStatus;
}

export interface Section {
  id: string;
  title: string;
  icon: IconName;
  /** One line on the tile. */
  blurb: string;
  detect: detect.Detect;
  component: ComponentType<SectionProps>;
}

export const SECTIONS: Section[] = [
  {
    id: "device",
    title: "Device",
    icon: "device",
    blurb: "Live status, hardware and settings",
    detect: detect.device,
    component: DeviceSection,
  },
  {
    id: "presence",
    title: "Presence",
    icon: "presence",
    blurb: "Sees people walk up to the screen",
    detect: detect.presence,
    component: PresenceSection,
  },
  {
    id: "camera",
    title: "Camera",
    icon: "camera",
    blurb: "Live preview and snapshots",
    detect: detect.camera,
    component: CameraSection,
  },
  {
    id: "remote",
    title: "TV remote",
    icon: "remote",
    blurb: "The TV and its remote over HDMI-CEC",
    detect: detect.remote,
    component: RemoteSection,
  },
  {
    id: "touch",
    title: "Touch & display",
    icon: "touch",
    blurb: "Multi-touch, scrolling and the screen",
    detect: detect.touch,
    component: TouchSection,
  },
  {
    id: "keyboard",
    title: "Keyboard & inputs",
    icon: "keyboard",
    blurb: "On-screen keyboard and every input",
    detect: detect.keyboard,
    component: KeyboardSection,
  },
  {
    id: "audio",
    title: "Audio",
    icon: "audio",
    blurb: "Playback, a synthesizer and the mic",
    detect: detect.audio,
    component: AudioSection,
  },
  {
    id: "video",
    title: "Video",
    icon: "video",
    blurb: "A film streamed from the internet",
    detect: detect.video,
    component: VideoSection,
  },
  {
    id: "printing",
    title: "Printing",
    icon: "printer",
    blurb: "Print a receipt from the page",
    detect: detect.printing,
    component: PrintingSection,
  },
  {
    id: "scripts",
    title: "Scripts",
    icon: "script",
    blurb: "Run the device's own scripts",
    detect: detect.scripts,
    component: ScriptsSection,
  },
  {
    id: "network",
    title: "Network",
    icon: "network",
    blurb: "The link, ping and a speed test",
    detect: detect.network,
    component: NetworkSection,
  },
  {
    id: "files",
    title: "Files & gallery",
    icon: "files",
    blurb: "The file store and a photo wall",
    detect: detect.files,
    component: FilesSection,
  },
  {
    id: "data",
    title: "Saved data",
    icon: "data",
    blurb: "A note that survives a reboot",
    detect: detect.data,
    component: DataSection,
  },
  {
    id: "peripherals",
    title: "Peripherals",
    icon: "plug",
    blurb: "Serial ports and HID devices",
    detect: detect.peripherals,
    component: PeripheralsSection,
  },
  {
    id: "graphics",
    title: "Graphics",
    icon: "graphics",
    blurb: "WebGL, fonts and emoji",
    detect: detect.graphics,
    component: GraphicsSection,
  },
  {
    id: "playlist",
    title: "Playlist",
    icon: "playlist",
    blurb: "What the player shows",
    detect: detect.playlist,
    component: PlaylistSection,
  },
  {
    id: "browser",
    title: "Browser control",
    icon: "browser",
    blurb: "Reload, maintenance and reboot",
    detect: detect.browser,
    component: BrowserSection,
  },
];

export function sectionById(id: string | undefined): Section | undefined {
  return SECTIONS.find((section) => section.id === id);
}
