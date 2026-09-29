// The page bridge's preamble: window.tessaro. Registered by the agent with
// Page.addScriptToEvaluateOnNewDocument, so it runs in every document before
// any of the page's own scripts (control/bridge.rs, docs/bridge.md). The
// agent fills in the __NAME__ tokens with JSON before registering it.
(() => {
  "use strict";
  const MODE = __MODE__;
  const SETTLE = __SETTLE__;
  const ORIGINS = __ORIGINS__;
  const BINDING = __BINDING__;

  // The binding exists in every frame. Take it before any page script can,
  // and give an iframe or another origin nothing at all.
  let raw = window[BINDING];
  try {
    delete window[BINDING];
  } catch (_) {}
  if (window !== window.top || !ORIGINS.includes(location.origin)) {
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

  const api = {
    mode: MODE,
    get config() {
      return config;
    },
    log: (level, message) => call("log", level, message),
    device: { status: () => call("device.status") },
    network: { status: () => call("network.status") },
    audio: { status: () => call("audio.status") },
    printer: { list: () => call("printer.list") },
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
    api.screen = {
      on: () => call("screen.on"),
      off: () => call("screen.off"),
    };
    api.files = { list: (path) => call("files.list", path) };
    Object.assign(api.printer, {
      print: async (job) => {
        const { data, ...rest } = job || {};
        const sent = data === undefined || data === null ? rest : { ...rest, data: await base64(data) };
        return call("printer.print", sent);
      },
    });
    api.data = {
      set: (name, value) => call("data.set", name, value),
      unset: (name) => call("data.unset", name),
    };
  }

  for (const group of Object.values(api)) {
    if (group && typeof group === "object") Object.freeze(group);
  }
  Object.defineProperty(window, "tessaro", {
    value: Object.freeze(api),
    enumerable: true,
  });
})();
