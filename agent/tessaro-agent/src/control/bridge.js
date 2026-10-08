// The page bridge's preamble: window.tessaro. Registered by the agent with
// Page.addScriptToEvaluateOnNewDocument, so it runs in every document before
// any of the page's own scripts (control/bridge.rs, docs/bridge.md). The
// agent fills in the __NAME__ tokens with JSON before registering it.
(() => {
  "use strict";
  const MODE = __MODE__;
  const PRINTING = __PRINTING__;
  const SETTLE = __SETTLE__;
  const ORIGINS = __ORIGINS__;
  const FRAME_ORIGINS = __FRAME_ORIGINS__;
  const BINDING = __BINDING__;

  // The binding exists in every frame. Take it before any page script can,
  // and give another origin nothing at all. A frame is answered only as an
  // item of the player page, directly in it, from an item's origin.
  let raw = window[BINDING];
  try {
    delete window[BINDING];
  } catch (_) {}
  const allowed =
    window === window.top
      ? ORIGINS.includes(location.origin)
      : window.parent === window.top && FRAME_ORIGINS.includes(location.origin);
  if (!allowed) {
    return;
  }

  let config = Object.freeze(__CONFIG__);
  const pending = new Map();
  let next = 0;

  const call = (name, ...args) =>
    new Promise((resolve, reject) => {
      if (typeof raw !== "function") {
        reject(new Error("tessaro: the bridge is not connected"));
        return;
      }
      next += 1;
      pending.set(next, { resolve, reject });
      raw(JSON.stringify({ id: next, name, args }));
    });

  // What only the agent calls, under a name that changes with every agent
  // start; reached with Runtime.evaluate.
  Object.defineProperty(window, SETTLE, {
    value: Object.freeze({
      settle(id, ok, value) {
        const waiting = pending.get(id);
        if (!waiting) return;
        pending.delete(id);
        if (ok) {
          waiting.resolve(value);
        } else {
          const error = new Error(value && value.message ? value.message : String(value));
          if (value && typeof value === "object") Object.assign(error, value);
          waiting.reject(error);
        }
      },
      config(values, changed) {
        config = Object.freeze(values);
        window.dispatchEvent(new CustomEvent("tessaro:config", { detail: { changed } }));
      },
      // What happened on the HDMI-CEC bus (screen.cec.page).
      cec(detail) {
        window.dispatchEvent(new CustomEvent("tessaro:cec", { detail: Object.freeze(detail) }));
      },
      // Someone arrived, left, came near or went far (camera.presence.page).
      presence(detail) {
        window.dispatchEvent(new CustomEvent("tessaro:presence", { detail: Object.freeze(detail) }));
      },
      // A frame's faces, while the page watches them.
      faces(detail) {
        window.dispatchEvent(new CustomEvent("tessaro:faces", { detail: Object.freeze(detail) }));
      },
      // A scan began or ended, a scanner came or went (scanner.page).
      scanner(detail) {
        window.dispatchEvent(new CustomEvent("tessaro:scanner", { detail: Object.freeze(detail) }));
      },
      // A new DevTools session to the same page: its binding replaces the
      // one that went with the old session.
      rebind() {
        raw = window[BINDING];
        try {
          delete window[BINDING];
        } catch (_) {}
      },
    }),
  });

  // printer.enable: window.print() is the agent's. It renders the page with
  // its print styles and sends it to the default printer, without a dialog:
  // Chromium's own printing needs GTK, which this build has none of. The
  // page's beforeprint and afterprint handlers run around it, as they would.
  if (PRINTING) {
    const print = () => {
      window.dispatchEvent(new Event("beforeprint"));
      call("page.print", document.title)
        .catch((err) => console.warn(`tessaro: window.print(): ${err.message}`))
        .finally(() => window.dispatchEvent(new Event("afterprint")));
    };
    Object.defineProperty(window, "print", { value: print, writable: false, configurable: false });
  }
  if (MODE === "off") {
    return;
  }

  // The renewal timer of presence.watch(), while the page watches.
  let watching = null;
  const api = {
    mode: MODE,
    get config() {
      return config;
    },
    log: (level, message) => call("log", level, message),
    device: { status: () => call("device.status") },
    network: { status: () => call("network.status") },
    audio: { status: () => call("audio.status") },
    printer: {
      list: () => call("printer.list"),
      jobs: (printer) => call("printer.jobs", printer),
    },
    scripts: { list: () => call("scripts.list") },
    scanner: { list: () => call("scanner.list") },
    playlist: { status: () => call("playlist.status") },
    screen: { show: () => call("screen.show") },
    presence: {
      status: () => call("presence.status"),
      // The faces come as tessaro:faces while the agent's lease is fresh:
      // renewed here every few seconds, so a page that goes away stops
      // them by itself (WATCH_LEASE in control/bridge.rs).
      watch: () => {
        if (watching === null) {
          watching = setInterval(() => call("presence.watch").catch(() => {}), 4000);
        }
        return call("presence.watch");
      },
      unwatch: () => {
        if (watching !== null) clearInterval(watching);
        watching = null;
        return call("presence.unwatch");
      },
    },
  };

  // A document for printer.print: text as UTF-8, or bytes, as base64.
  const base64 = async (data) => {
    let bytes;
    if (typeof data === "string") bytes = new TextEncoder().encode(data);
    else if (data instanceof Blob) bytes = new Uint8Array(await data.arrayBuffer());
    else if (data instanceof ArrayBuffer) bytes = new Uint8Array(data);
    else if (ArrayBuffer.isView(data))
      bytes = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
    else throw new Error("tessaro: print data is a string, a Blob, an ArrayBuffer or bytes");
    let text = "";
    for (let at = 0; at < bytes.length; at += 0x8000) {
      text += String.fromCharCode.apply(null, bytes.subarray(at, at + 0x8000));
    }
    return btoa(text);
  };

  if (MODE === "actions") {
    Object.assign(api.device, { reboot: () => call("device.reboot") });
    Object.assign(api.network, {
      publicIp: () => call("network.publicIp"),
      online: () => call("network.online"),
      ping: (host) => call("network.ping", host),
      speedTest: () => call("network.speedTest"),
    });
    Object.assign(api.audio, {
      volume: (percent) => call("audio.volume", percent),
      mute: (on) => call("audio.mute", on),
      inputVolume: (percent) => call("audio.inputVolume", percent),
    });
    api.browser = {
      reload: () => call("browser.reload"),
      restart: () => call("browser.restart"),
      home: () => call("browser.home"),
      clearCache: () => call("browser.clearCache"),
      maintenance: (on, url) => call("browser.maintenance", on, url),
    };
    api.keyboard = {
      show: (selector) => call("keyboard.show", selector),
      hide: () => call("keyboard.hide"),
    };
    Object.assign(api.screen, {
      on: () => call("screen.on"),
      off: () => call("screen.off"),
      // The HDMI-CEC bus: the TV, its remote, and any message.
      cec: Object.freeze({
        wake: (source) => call("screen.cec.wake", source),
        standby: (all) => call("screen.cec.standby", all),
        source: () => call("screen.cec.source"),
        key: (key, to) => call("screen.cec.key", key, to),
        scan: () => call("screen.cec.scan"),
        send: (data, to, reply) => call("screen.cec.send", data, to, reply),
        messages: (after) => call("screen.cec.messages", after),
      }),
    });
    api.files = { list: (path) => call("files.list", path) };
    Object.assign(api.printer, {
      print: async (job) => {
        const { data, ...rest } = job || {};
        const sent = data === undefined || data === null ? rest : { ...rest, data: await base64(data) };
        return call("printer.print", sent);
      },
      cancel: (job) => call("printer.cancel", job),
    });
    api.data = {
      set: (name, value) => call("data.set", name, value),
      unset: (name) => call("data.unset", name),
    };
    Object.assign(api.scripts, { run: (name) => call("scripts.run", name) });
  }

  for (const group of Object.values(api)) {
    if (group && typeof group === "object") Object.freeze(group);
  }
  Object.defineProperty(window, "tessaro", {
    value: Object.freeze(api),
    enumerable: true,
  });

  // A frame of the player gets tessaro:scanner too: the agent reaches it in
  // the context this call comes from, until that context is gone.
  if (window !== window.top) {
    call("scanner.listen").catch(() => {});
  }
})();
