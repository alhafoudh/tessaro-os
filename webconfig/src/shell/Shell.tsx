// Webconfig's window, laid out as a GUI device window (device.rs): the
// title bar with the device tools, the menu on the left (a drawer on a
// phone), the page with the live Screen panel beside it where the GUI has
// its VNC panel, the Messages pane and the status bar.

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
    body: (name) =>
      `Restart the compositor on ${name}? The browser and the agent restart with it, and this browser is signed out.`,
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
        `block px-2.5 py-1 whitespace-nowrap ${isActive ? "bg-selection text-white" : "hover:bg-button-hover"}`
      }
    >
      {title}
    </NavLink>
  );

  return (
    <div className="flex h-full flex-col bg-background">
      <header className="flex min-h-[2.125rem] flex-wrap items-center gap-x-3 gap-y-1 border-b border-border bg-chrome px-2 py-1">
        <button
          type="button"
          className="px-1.5 text-xl leading-none md:hidden"
          aria-label="Menu"
          aria-expanded={menu}
          onClick={() => setMenu((open) => !open)}
        >
          ☰
        </button>
        <span className="text-[0.9375rem] font-bold">Tessaro</span>
        <span className="text-muted">Webconfig</span>
        <span className="truncate font-bold">{status?.node.name}</span>
        <div className="ms-auto flex flex-wrap items-center gap-1.5">
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
          className={`${menu ? "flex" : "hidden"} absolute inset-y-0 left-0 z-40 w-48 flex-col overflow-auto border-r border-border bg-panel md:static md:flex md:w-[140px] md:shrink-0`}
          aria-label="Pages"
        >
          {PAGES.map((page) => link(`/${page.path}`, page.title))}
          {sections.length > 0 && <div className="mx-2 my-1 h-px bg-border" />}
          {sections.map((section) => link(`/settings/${section}`, sectionTitle(section)))}
        </nav>
        {menu && <div className="absolute inset-0 z-30 bg-black/45 md:hidden" onClick={() => setMenu(false)} />}
        {/* The screen beside the page, as the GUI's VNC panel; under it on a phone. */}
        <div className="flex min-w-0 flex-1 flex-col md:flex-row">
          <main className="min-h-0 min-w-0 flex-1 overflow-auto p-2">{children}</main>
          {showScreen && <ScreenPanel onClose={() => setShowScreen(false)} />}
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
