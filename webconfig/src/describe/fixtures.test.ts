// The TypeScript port of the result text against the golden fixtures the
// Rust records (agent/client/tests/describe.rs): same input, same spans.
// A fixture names a function this map does not have yet: add its port.

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { renderers } from "./renderers";

const dir = join(__dirname, "../../../agent/client/tests/describe");

describe("the result text", () => {
  const files = readdirSync(dir).filter((name) => name.endsWith(".json"));

  it("has fixtures", () => {
    expect(files.length).toBeGreaterThan(0);
  });

  for (const file of files) {
    it(`renders ${file} as the Rust does`, () => {
      const fixture = JSON.parse(readFileSync(join(dir, file), "utf8")) as {
        function: string;
        input: unknown;
        output: unknown;
      };
      const render = renderers[fixture.function];
      expect(render, `no port of ${fixture.function} in renderers.ts`).toBeDefined();
      expect(render!(fixture.input)).toEqual(fixture.output);
    });
  }
});
