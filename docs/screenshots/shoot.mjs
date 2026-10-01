// Renders the README's screenshots from the pages as they ship and fixed
// data, so the pictures follow the layouts and never show a real device.
// `mise run docs:screenshots` writes the agent's fixtures (welcome.json,
// debug.html) and runs this in the Playwright image, which has the browser
// and its libraries.

import { readFileSync } from "node:fs";
import { chromium } from "playwright-core";

const repo = new URL("../../", import.meta.url).pathname;
const read = (path) => readFileSync(`${repo}${path}`, "utf8");
const selftest = "meta-tessaro-distro/recipes-browser/tessaro-selftest/files";

// Each screen as the kiosk loads it: the URL, and what answers each request.
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
];

const browser = await chromium.launch();
for (const shot of shots) {
  // The pages are laid out for a 1920x1080 panel and saved 1600 wide.
  const page = await browser.newPage({
    viewport: { width: 1920, height: 1080 },
    deviceScaleFactor: 1600 / 1920,
  });
  await page.route("**/*", (route) => {
    const url = route.request().url().split("?")[0];
    const answer = shot.routes[url];
    return answer
      ? route.fulfill({ contentType: answer[0], body: answer[1] })
      : route.abort();
  });
  await page.goto(shot.url);
  if (shot.ready) await page.waitForFunction(shot.ready);
  await page.evaluate(() => document.fonts.ready);
  const path = `docs/images/${shot.name}.jpg`;
  await page.screenshot({ path: `${repo}${path}`, type: "jpeg", quality: 85, animations: "disabled" });
  await page.close();
  console.log(path);
}
await browser.close();
