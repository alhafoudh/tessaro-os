// Webconfig's window, laid out as a GUI device window (device.rs): the
// title bar with the device tools, the menu on the left (a drawer on a
// phone, which holds the tools there too), the page with the live Screen panel beside it where the GUI has
// its VNC panel and the camera panel under it, the Messages pane and the status bar.

import { useEffect, useState, type ReactNode } from "react";
import { NavLink, useLocation } from "react-router";

import { answer, client, failure, type Schemas } from "../api/client";
import { touched } from "../api/activity";
import { useDevice } from "../device/DeviceContext";
import { CLAIMED, PAGES, sectionTitle } from "../pages/registry";
import { useSession } from "../session/SessionContext";
import { ownSections } from "../settings/scope";
import { Button, LineView } from "../ui/controls";
import { Confirm } from "../ui/dialogs";
import { CameraPanel, CameraPanelContext } from "./CameraPanel";
import { ScreenPanel, useScreenPanel } from "./ScreenPanel";
import { StatusBar } from "./StatusBar";

type Restart = Schemas["RestartTarget"] | "reboot";

const RESTARTS: Record<Restart, { title: string; body: (name: string) => string; label: string }> = {
  browser: {
    title: "Restart the browser",
    body: (name) => `Restart the browser on ${name}? The screen goes blank for a moment.`,
    label: "Restart",
  },
  weston: {
    title: "Restart weston",
    body: (name) => `Restart the compositor on ${name}? The browser restarts with it; this browser stays signed in.`,
    label: "Restart",
  },
  agent: {
    title: "Restart the agent",
    body: (name) =>
      `Restart the agent on ${name}? Nothing changes on screen; this browser is signed out and signs in again.`,
    label: "Restart",
  },
  reboot: {
    title: "Reboot",
    body: (name) => `Reboot ${name}? The screen is dark until it is back, a minute or so.`,
    label: "Reboot",
  },
};

export function Shell({ children }: { children: ReactNode }) {
  const { status, settings, messages, clearMessages, refresh, log } = useDevice();
  const { session, signOut } = useSession();
  const [menu, setMenu] = useState(false);
  // Open where there is room for it beside the page; a phone opens it from
  // the title bar.
  const [showMessages, setShowMessages] = useState(() => window.matchMedia("(min-width: 768px)").matches);
  const [showScreen, setShowScreen] = useScreenPanel();
  // The camera the camera panel shows, the one last double-clicked on the
  // Camera page; closed on a reload.
  const [camera, setCamera] = useState<string | null>(null);
  const [asking, setAsking] = useState<Restart | null>(null);
  const location = useLocation();
  const name = status?.node.name ?? "the device";

  // A navigation is use of the session, as an input is.
  useEffect(() => {
    touched();
    setMenu(false);
  }, [location.pathname]);

  const restart = async (what: Restart) => {
    try {
      const done =
        what === "reboot"
          ? await answer(client.POST("/api/v1/device/reboot"))
          : await answer(client.POST("/api/v1/device/restart", { body: { what } }));
      log(done.message, "ok");
    } catch (error) {
      log(failure(error).message, "bad");
    }
  };

  const sections = settings ? ownSections(settings, CLAIMED) : [];
  const link = (to: string, title: string) => (
    <NavLink
      key={to}
      to={to}
      className={({ isActive }) =>
        `block px-2.5 py-1 whitespace-nowrap max-md:py-2 ${isActive ? "bg-selection text-white" : "hover:bg-button-hover"}`
      }
    >
      {title}
    </NavLink>
  );

  // A phone has no room for the title bar's tools: they are the drawer's
  // last entries there, and close it as a page link does.
  const drawerTool = (title: string, run: () => void, on?: boolean) => (
    <button
      type="button"
      className={`block w-full px-2.5 py-2 text-left whitespace-nowrap hover:bg-button-hover ${on ? "text-primary" : ""}`}
      aria-pressed={on}
      onClick={() => {
        setMenu(false);
        run();
      }}
    >
      {title}
      {on !== undefined && <span className="text-muted">{on ? ": shown" : ""}</span>}
    </button>
  );

  return (
    <div className="flex h-full flex-col bg-background">
      <header className="flex min-h-[2.125rem] flex-wrap items-center gap-x-3 gap-y-1 border-b border-border bg-chrome px-2 py-1 max-md:flex-nowrap max-md:gap-x-2">
        <button
          type="button"
          className="p-1 md:hidden"
          aria-label="Menu"
          aria-expanded={menu}
          onClick={() => setMenu((open) => !open)}
        >
          {/* Drawn, not the ☰ character: Manrope has no such glyph, and each
              phone's fallback font puts it at its own height. */}
          <svg viewBox="0 0 20 20" className="block size-5" aria-hidden>
            <path d="M3 5h14M3 10h14M3 15h14" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
          </svg>
        </button>
        {/* One baseline for the differently sized names. */}
        <span className="flex min-w-0 items-baseline gap-x-3 max-md:flex-1 max-md:gap-x-2">
          <span className="text-[0.9375rem] font-bold">Tessaro</span>
          <span className="text-muted max-md:hidden">Webconfig</span>
          <span className="min-w-0 truncate font-bold">{status?.node.name}</span>
        </span>
        <Button className="md:hidden" onClick={refresh}>
          Refresh
        </Button>
        <div className="ms-auto flex flex-wrap items-center gap-1.5 max-md:hidden">
          <Button onClick={refresh}>Refresh</Button>
          <span className="mx-0.5 h-4 w-px bg-border" />
          <Button onClick={() => setAsking("browser")}>Restart browser</Button>
          <Button onClick={() => setAsking("weston")}>Restart weston</Button>
          <Button onClick={() => setAsking("agent")}>Restart agent</Button>
          <Button onClick={() => setAsking("reboot")}>Reboot</Button>
          <span className="mx-0.5 h-4 w-px bg-border" />
          <Button
            kind={showScreen ? "primary" : "tool"}
            onClick={() => setShowScreen(!showScreen)}
            aria-pressed={showScreen}
          >
            Screen
          </Button>
          <Button onClick={() => setShowMessages((open) => !open)} aria-pressed={showMessages}>
            Messages
          </Button>
          {session.via === "session" && <Button onClick={() => void signOut()}>Sign out</Button>}
        </div>
      </header>
      <div className="relative flex min-h-0 flex-1">
        <nav
          className={`${menu ? "flex" : "hidden"} absolute inset-y-0 left-0 z-40 w-60 max-w-[85vw] flex-col overflow-auto border-r border-border bg-panel md:static md:flex md:w-[140px] md:shrink-0`}
          aria-label="Pages"
        >
          {PAGES.map((page) => link(`/${page.path}`, page.title))}
          {sections.length > 0 && <div className="mx-2 my-1 h-px bg-border" />}
          {sections.map((section) => link(`/settings/${section}`, sectionTitle(section)))}
          <div className="md:hidden">
            <div className="mx-2 my-1 h-px bg-border" />
            {drawerTool("Screen", () => setShowScreen(!showScreen), showScreen)}
            {drawerTool("Messages", () => setShowMessages((open) => !open), showMessages)}
            <div className="mx-2 my-1 h-px bg-border" />
            {drawerTool("Restart browser", () => setAsking("browser"))}
            {drawerTool("Restart weston", () => setAsking("weston"))}
            {drawerTool("Restart agent", () => setAsking("agent"))}
            {drawerTool("Reboot", () => setAsking("reboot"))}
            {session.via === "session" && drawerTool("Sign out", () => void signOut())}
          </div>
        </nav>
        {menu && <div className="absolute inset-0 z-30 bg-black/45 md:hidden" onClick={() => setMenu(false)} />}
        {/* The screen and the camera beside the page, as the GUI's VNC and
            camera panels, one above the other; under the page on a phone. */}
        <div className="flex min-w-0 flex-1 flex-col md:flex-row">
          <main className="min-h-0 min-w-0 flex-1 overflow-auto p-2">
            <CameraPanelContext.Provider value={setCamera}>{children}</CameraPanelContext.Provider>
          </main>
          {(showScreen || camera) && (
            <div className="flex max-h-[45vh] min-h-0 flex-col divide-y divide-border border-t border-border bg-panel md:max-h-none md:w-[40%] md:max-w-[900px] md:min-w-[280px] md:border-t-0 md:border-l">
              {showScreen && <ScreenPanel onClose={() => setShowScreen(false)} />}
              {camera && <CameraPanel key={camera} device={camera} onClose={() => setCamera(null)} />}
            </div>
          )}
        </div>
      </div>
      {showMessages && (
        <div className="border-t border-border bg-panel px-1.5 py-1">
          <div className="flex items-center">
            <span className="text-sm font-bold">Messages</span>
            <Button className="ms-auto" onClick={clearMessages}>
              Clear
            </Button>
          </div>
          <div className="mt-0.5 h-[6.875rem] overflow-auto font-mono text-sm whitespace-pre-wrap" ref={scrollToEnd}>
            {messages.map((line, at) => (
              <div key={at}>
                <LineView line={line} />
              </div>
            ))}
          </div>
        </div>
      )}
      <StatusBar />
      {asking && (
        <Confirm
          title={RESTARTS[asking].title}
          body={RESTARTS[asking].body(name)}
          action={RESTARTS[asking].label}
          danger={asking === "reboot"}
          onConfirm={() => void restart(asking)}
          onClose={() => setAsking(null)}
        />
      )}
    </div>
  );
}

/** Keep a log pane on its newest line. */
function scrollToEnd(element: HTMLDivElement | null) {
  if (element) {
    element.scrollTop = element.scrollHeight;
  }
}
