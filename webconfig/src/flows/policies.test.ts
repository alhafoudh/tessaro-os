import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { check, checkName, errorText, nameOf, strictJson, TEMPLATE } from "./policies";

describe("a browser policy as agent/protocol/src/policy.rs checks it", () => {
  it("keeps comments and trailing commas in their place", () => {
    const text = '{\n  /* one\n     two */\n  "A": 1, // x\n  "B": [1, 2,],\n}';
    const strict = strictJson(text);
    expect(strict.length).toBe(text.length);
    expect(strict.split("\n").length).toBe(text.split("\n").length);
    expect(JSON.parse(strict)).toEqual({ A: 1, B: [1, 2] });
  });

  it("leaves strings alone", () => {
    const value = JSON.parse(strictJson('{"Url": "http://a.test/*,", "Esc": "a\\"//b"}'));
    expect(value.Url).toBe("http://a.test/*,");
    expect(value.Esc).toBe('a"//b');
  });

  it("points a syntax error into the text as typed", () => {
    const checked = check('{\n  /* a\n     comment */\n  "A": 1\n  "B": 2\n}');
    expect("error" in checked && [checked.error.line, checked.error.column]).toEqual([5, 3]);
    expect("error" in checked && errorText(checked.error)).toMatch(/^line 5 column 3: /);
  });

  it("refuses what the device sets itself, with its command", () => {
    const say = (text: string) => {
      const checked = check(text);
      return "error" in checked ? checked.error.message : "";
    };
    expect(say('{"CACertificates": []}')).toBe(
      "CACertificates is set by the device itself; use `tessaro-ctl network certs add FILE`",
    );
    expect(say('{"ProxyMode": "direct"}')).toContain("tessaro-ctl network proxy set");
    expect(say('{"SerialAllowAllPortsForUrls": []}')).toContain("browser.device_origins");
  });

  it("takes only an object of policy names", () => {
    expect("error" in check("[]")).toBe(true);
    expect("error" in check('{"URL Blocklist": []}')).toBe(true);
    expect("error" in check('{"1Bad": []}')).toBe(true);
    expect(check('{"B": 1, "A": 2}')).toEqual({ keys: ["A", "B"] });
  });

  it("starts from the device's own template", () => {
    const file = join(__dirname, "../../../agent/protocol/src/policy-template.jsonc");
    expect(TEMPLATE).toBe(readFileSync(file, "utf8"));
    expect(check(TEMPLATE)).toEqual({
      keys: ["DownloadRestrictions", "PasswordManagerEnabled", "SpellcheckEnabled", "URLAllowlist", "URLBlocklist"],
    });
  });

  it("names", () => {
    expect(checkName("lockdown")).toBeNull();
    expect(checkName("corp-urls_2")).toBeNull();
    expect(checkName("")).not.toBeNull();
    expect(checkName("-x")).not.toBeNull();
    expect(checkName("Upper")).not.toBeNull();
    expect(checkName("a".repeat(33))).not.toBeNull();
    expect(nameOf("Corp Lockdown.v2.json")).toBe("corp-lockdown-v2");
  });
});
