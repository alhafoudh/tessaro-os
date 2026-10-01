// Renders the README's screenshot of the welcome page from the page as it
// ships and a fixed welcome.json, so the picture follows the layout and never
// shows a real device. `mise run docs:screenshots` writes the fixture and runs
// this in the Playwright image, which has the browser and its libraries.

import { readFileSync } from "node:fs";
import { chromium } from "playwright-core";

const repo = new URL("../../", import.meta.url).pathname;
const page_html = readFileSync(
  `${repo}meta-tessaro-distro/recipes-browser/tessaro-selftest/files/index.html`,
  "utf8",
);
const welcome = readFileSync(`${repo}docs/screenshots/welcome.json`, "utf8");

const browser = await chromium.launch();
// The page is laid out for a 1920x1080 panel and saved 1600 wide.
const page = await browser.newPage({
  viewport: { width: 1920, height: 1080 },
  deviceScaleFactor: 1600 / 1920,
});
await page.route("http://127.0.0.1/welcome.json", (route) =>
  route.fulfill({ contentType: "application/json", body: welcome }),
);
await page.route("http://127.0.0.1/", (route) =>
  route.fulfill({ contentType: "text/html", body: page_html }),
);
await page.goto("http://127.0.0.1/");
await page.waitForFunction(() => document.getElementById("node").textContent !== "-");
await page.evaluate(() => document.fonts.ready);
await page.screenshot({ path: `${repo}docs/images/welcome.jpg`, type: "jpeg", quality: 85 });
await browser.close();
console.log("docs/images/welcome.jpg");
