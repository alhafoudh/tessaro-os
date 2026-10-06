// Renders the README's screenshots from the pages as they ship and fixed
// data, so the pictures follow the layouts and never show a real device.
// `mise run docs:screenshots` writes the agent's fixtures (welcome.json,
// debug.html, api/), builds Webconfig and runs this in the Playwright image,
// which has the browser and its libraries.

import { existsSync, readFileSync, statSync } from "node:fs";
import { extname } from "node:path";
import { chromium } from "playwright-core";

const repo = new URL("../../", import.meta.url).pathname;
const read = (path) => readFileSync(`${repo}${path}`, "utf8");
const selftest = "meta-tessaro-distro/recipes-browser/tessaro-selftest/files";

const TYPES = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".woff2": "font/woff2",
  ".ttf": "font/ttf",
  ".wasm": "application/wasm",
};

/**
 * Webconfig as the device serves it: the build's files, index.html for any
 * other page (api/statics.rs), and the API's answers from the made-up
 * device's fixtures. A request without one is named, so a page that starts
 * asking for more shows up here instead of as an empty corner of a picture.
 */
function webconfig(url) {
  const path = new URL(url).pathname;
  if (path.startsWith("/api/")) {
    const fixture = path === "/api/v1/device/welcome" ? "welcome.json" : `${path.slice(1)}.json`;
    if (existsSync(`${repo}docs/screenshots/${fixture}`)) {
      return ["application/json", read(`docs/screenshots/${fixture}`)];
    }
    console.error(`no fixture for ${path}`);
    return [
      "application/json",
      JSON.stringify({ error: `no fixture for ${path}` }),
      404,
    ];
  }
  const file = `${repo}build/webconfig${path}`;
  const found = existsSync(file) && statSync(file).isFile();
  const served = found ? file : `${repo}build/webconfig/index.html`;
  return [TYPES[extname(served)] ?? "application/octet-stream", readFileSync(served)];
}

/** Webconfig waits for the device before it shows anything of it. */
const answered = () =>
  !!document.querySelector("main") &&
  !document.body.innerText.includes("asking the device") &&
  !document.body.innerText.includes("Checking ...");

// Each screen as the kiosk or a browser loads it: the URL, and what answers
// each request.
const shots = [
  {
    // The welcome page polls welcome.json, which the agent writes.
    name: "welcome",
    url: "http://127.0.0.1/",
    routes: {
      "http://127.0.0.1/": ["text/html", read(`${selftest}/index.html`)],
      "http://127.0.0.1/welcome.json": ["application/json", read("docs/screenshots/welcome.json")],
    },
    ready: () => document.getElementById("node").textContent !== "-",
  },
  {
    // The message is the README's example.
    name: "maintenance",
    url:
      "http://127.0.0.1/maintenance.html?message=" +
      encodeURIComponent("We are restocking the shelves. Back at 14:00."),
    routes: {
      "http://127.0.0.1/maintenance.html": ["text/html", read(`${selftest}/maintenance.html`)],
    },
  },
  {
    // The agent writes the debug screen as a page of its own.
    name: "debug-screen",
    url: "http://127.0.0.1/debug.html",
    routes: {
      "http://127.0.0.1/debug.html": ["text/html", read("docs/screenshots/debug.html")],
    },
  },
  // tessaro-gui draws itself headless (screenshot.rs), at twice the size of
  // its 1400x860 window; here it only becomes a JPEG like the others.
  ...["gui-overview", "gui-configure"].map((name) => ({
    name,
    url: "http://127.0.0.1/",
    routes: {
      "http://127.0.0.1/": [
        "text/html",
        '<body style="margin:0"><img src="/shot.png" style="width:1400px;height:860px;display:block">',
      ],
      "http://127.0.0.1/shot.png": ["image/png", readFileSync(`${repo}build/screenshots/${name}-tiny-skia.png`)],
    },
    ready: () => document.querySelector("img").complete,
    gui: true,
  })),
  {
    name: "webconfig-overview",
    url: "https://192.168.1.42:7400/overview",
    routes: webconfig,
    ready: answered,
    browser: true,
  },
  {
    name: "webconfig-playlists",
    url: "https://192.168.1.42:7400/playlists",
    routes: webconfig,
    ready: answered,
    browser: true,
    // 42 s after the item in the fixture came on screen, so its "ago" is
    // the same in every picture.
    clock: (1_791_270_000 + 42) * 1000,
  },
  {
    name: "webconfig-quick-setup",
    url: "https://192.168.1.42:7400/quick-setup",
    routes: webconfig,
    ready: answered,
    browser: true,
  },
];

const browser = await chromium.launch();
let missing = false;
for (const shot of shots) {
  // The kiosk's pages are laid out for a 1920x1080 panel, Webconfig for a
  // laptop's browser, tessaro-gui for its window; all are saved 1600 wide.
  const [width, height] = shot.gui ? [1400, 860] : shot.browser ? [1280, 800] : [1920, 1080];
  const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 1600 / width });
  await page.route("**/*", (route) => {
    const url = route.request().url().split("?")[0];
    const answer = typeof shot.routes === "function" ? shot.routes(url) : shot.routes[url];
    if (answer?.[2]) missing = true;
    return answer
      ? route.fulfill({ contentType: answer[0], body: answer[1], status: answer[2] ?? 200 })
      : route.abort();
  });
  if (shot.clock) await page.clock.setFixedTime(new Date(shot.clock));
  await page.goto(shot.url);
  if (shot.ready) await page.waitForFunction(shot.ready);
  await page.evaluate(() => document.fonts.ready);
  const path = `docs/images/${shot.name}.jpg`;
  await page.screenshot({ path: `${repo}${path}`, type: "jpeg", quality: 85, animations: "disabled" });
  await page.close();
  console.log(path);
}
await browser.close();
if (missing) process.exit(1);
