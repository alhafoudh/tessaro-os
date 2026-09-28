// The GUI's Browser page (pages.rs browser_view): what the browser shows
// and the `browser` commands. DevTools is left out: it is an SSH tunnel to
// the device, which only the native clients open. A setting a form changes
// is checked by the device at `config set`, and its refusal shows in the
// form, next to what was typed.

import { useState } from "react";

import { answer, client, failure } from "../api/client";
import * as describe from "../describe/device";
import { useDevice } from "../device/DeviceContext";
import { PageFrame } from "../shell/PageFrame";
import { Line, fact } from "../text/line";
import { Button, Facts, Output } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import type { PageInfo } from "./registry";

/** protocol::keys::BRIDGE_MODES. */
const BRIDGE_MODES = ["off", "config", "actions"];

type Form = "navigate" | "maintenance" | "debug" | "zoom" | "inject" | "bridge" | "eval";

interface Spec {
  title: string;
  submit: string;
  intro: string;
  label: string;
  placeholder: string;
  initial: string;
  choices?: string[];
  /** Run it; a thrown error is shown in the form, which stays open. */
  run: (value: string) => Promise<void>;
}

export function Browser({ info }: { info: PageInfo }) {
  const { status, settings, link, set, log, refresh } = useDevice();
  const online = link === "online";
  const [form, setForm] = useState<Form | null>(null);
  const [output, setOutput] = useState<Line[]>([]);

  const setting = (key: string) => settings?.settings.find((item) => item.key === key)?.value ?? "";
  const zoom = setting("browser.zoom") || "100";

  const done = async (pending: Promise<{ message: string }>) => {
    try {
      log((await pending).message, "ok");
    } catch (error) {
      log(failure(error).message, "bad");
    }
    refresh();
  };

  const facts = status
    ? [
        fact("Kiosk page", status.kiosk_url),
        fact("Showing", status.current_url ?? ""),
        fact("Browser answers", status.browser_answering ? "yes" : "no"),
        fact("Maintenance", status.maintenance ? "yes" : "no"),
        fact("Debug screen", status.debug_screen ? "yes" : "no"),
        fact("Page zoom", `${zoom}%`),
        fact("DevTools", status.devtools ? "connected - the agent leaves the tab alone" : "not connected"),
        ...(status.bridge
          ? [
              fact("Page bridge", status.bridge.mode),
              fact(
                "Injected script",
                !status.bridge.script
                  ? "none"
                  : status.bridge.script_problem
                    ? `${status.bridge.script} (not injected: ${status.bridge.script_problem})`
                    : status.bridge.script,
              ),
            ]
          : []),
      ]
    : [];

  const maintenance = !!status?.maintenance;
  const debug = !!status?.debug_screen;
  const currentBridge = setting("browser.bridge.mode");

  const specs: Record<Form, Spec> = {
    navigate: {
      title: "Navigate",
      submit: "Go",
      intro: "Open a page now. The kiosk page is back after a restart; set browser.url to change it for good.",
      label: "URL",
      placeholder: "https://...",
      initial: status?.current_url ?? "",
      run: async (url) => {
        if (!url) throw new Error("a URL, please");
        await done(answer(client.POST("/api/v1/browser/navigate", { body: { url } })));
      },
    },
    maintenance: {
      title: "Maintenance mode",
      submit: "Turn on",
      intro: "The screen shows the maintenance page until it is turned off.",
      label: "Page",
      placeholder: "the image's maintenance page",
      initial: setting("browser.maintenance.url"),
      run: async (url) => {
        await set({ "browser.maintenance.enable": "1", ...(url ? { "browser.maintenance.url": url } : {}) });
      },
    },
    debug: {
      title: "Debug screen",
      submit: "Turn on",
      intro: "The screen shows the device's details until it is turned off. {key} placeholders work as in browser.url.",
      label: "Template",
      placeholder: "the image's template",
      initial: setting("browser.debug.template"),
      run: async (template) => {
        await set({ "browser.debug.enable": "1", ...(template ? { "browser.debug.template": template } : {}) });
      },
    },
    zoom: {
      title: "Page zoom",
      submit: "Zoom",
      intro: "Percent, 25 to 500: Chrome's Ctrl+/- zoom for every site. 100 is no zoom. The browser restarts.",
      label: "Percent",
      placeholder: "100",
      initial: zoom,
      run: async (percent) => {
        await set({ "browser.zoom": percent });
      },
    },
    inject: {
      title: "Inject a script",
      submit: "Save",
      intro:
        "A file from the file store, run in every page before the page's own scripts. Uploading a new copy reloads the page with it. Empty for none.",
      label: "Script",
      placeholder: "inject.js",
      initial: setting("browser.inject.script"),
      run: async (script) => {
        await set({ "browser.inject.script": script });
      },
    },
    bridge: {
      title: "Page bridge",
      submit: "Save",
      intro:
        "What the page gets as window.tessaro: nothing, the settings (config), or the settings and device actions such as reload, volume and screen power (actions).",
      label: "Mode",
      placeholder: "",
      initial: BRIDGE_MODES.includes(currentBridge) ? currentBridge : "off",
      choices: BRIDGE_MODES,
      run: async (mode) => {
        await set({ "browser.bridge.mode": mode });
      },
    },
    eval: {
      title: "Run JavaScript",
      submit: "Run",
      intro: "Runs in the page on screen now, as tessaro-ctl browser eval. What it returns goes to the output below.",
      label: "Code",
      placeholder: "document.title",
      initial: "",
      run: async (code) => {
        if (!code) throw new Error("some code, please");
        setOutput((lines) => [...lines, Line.plain(`> ${code}`)]);
        try {
          const result = await answer(
            client.POST("/api/v1/browser/eval", {
              body: { code, timeout_ms: null, await_promise: true, user_gesture: false },
            }),
          );
          const { line } = describe.evalResult(result);
          // A pretty-printed value is one span over several lines.
          const shown =
            line.spans.length === 1 && line.spans[0]!.text.includes("\n")
              ? line.spans[0]!.text.split("\n").map((part) => Line.of(line.spans[0]!.tone, part))
              : [line];
          setOutput((lines) => [...lines, ...shown]);
        } catch (error) {
          setOutput((lines) => [...lines, Line.of("bad", failure(error).message)]);
        }
      },
    },
  };

  return (
    <PageFrame
      title={info.title}
      scope={info.scope}
      tools={
        <>
          <Button disabled={!online} onClick={() => setForm("navigate")}>
            Navigate
          </Button>
          <Button
            disabled={!online}
            onClick={() =>
              maintenance
                ? void set({ "browser.maintenance.enable": "0" }).catch(() => undefined)
                : setForm("maintenance")
            }
          >
            {maintenance ? "Maintenance off" : "Maintenance on"}
          </Button>
          <Button
            disabled={!online}
            onClick={() =>
              debug ? void set({ "browser.debug.enable": "0" }).catch(() => undefined) : setForm("debug")
            }
          >
            {debug ? "Debug screen off" : "Debug screen on"}
          </Button>
          <Button disabled={!online} onClick={() => setForm("zoom")}>
            Zoom
          </Button>
          <Button disabled={!online} onClick={() => void done(answer(client.POST("/api/v1/browser/reload")))}>
            Reload
          </Button>
          <Button disabled={!online} onClick={() => void done(answer(client.POST("/api/v1/browser/clear-cache")))}>
            Clear cache
          </Button>
          <Button disabled={!online} onClick={() => setForm("inject")}>
            Inject
          </Button>
          <Button disabled={!online} onClick={() => setForm("bridge")}>
            Bridge
          </Button>
          <Button disabled={!online} onClick={() => setForm("eval")}>
            Run JavaScript
          </Button>
        </>
      }
    >
      {status ? <Facts facts={facts} /> : <p className="text-sm text-muted">asking the device ...</p>}
      <Output lines={output} />
      {form && <FormDialog key={form} spec={specs[form]} onClose={() => setForm(null)} />}
    </PageFrame>
  );
}

function FormDialog({ spec, onClose }: { spec: Spec; onClose: () => void }) {
  // Taken once: a refresh underneath does not overwrite what is typed.
  const [value, setValue] = useState(spec.initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      await spec.run(value.trim());
      onClose();
    } catch (problem) {
      setError(problem instanceof Error && !("code" in problem) ? problem.message : failure(problem).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog title={spec.title} onClose={onClose} onSubmit={() => void submit()} submit={spec.submit} busy={busy}>
      <Intro>{spec.intro}</Intro>
      <Field label={spec.label}>
        {spec.choices ? (
          <select value={value} onChange={(event) => setValue(event.target.value)}>
            {spec.choices.map((choice) => (
              <option key={choice} value={choice}>
                {choice}
              </option>
            ))}
          </select>
        ) : (
          <input
            value={value}
            placeholder={spec.placeholder}
            onChange={(event) => setValue(event.target.value)}
            spellCheck={false}
            autoCapitalize="off"
            className={spec.label === "Code" ? "font-mono" : ""}
          />
        )}
      </Field>
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
  );
}
