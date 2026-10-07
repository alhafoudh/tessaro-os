import { describe, expect, it } from "vitest";

import { endMaintenance, type EndStep } from "./maintenance";

function refusingFor(times: number) {
  let calls = 0;
  return {
    calls: () => calls,
    bridge: {
      browser: {
        reload: async () => null,
        restart: async () => null,
        home: async () => null,
        clearCache: async () => null,
        maintenance: async (on: boolean) => {
          calls += 1;
          expect(on).toBe(false);
          if (calls <= times) {
            throw new Error(`refused: the page was started over 5s ago; try again in ${55 - calls}s`);
          }
          return null;
        },
      },
    },
  };
}

describe("endMaintenance", () => {
  it("switches maintenance off at once when the agent lets it", async () => {
    const fake = refusingFor(0);
    const steps: EndStep[] = [];
    await endMaintenance(
      fake.bridge,
      (step) => steps.push(step),
      async () => {},
    );
    expect(fake.calls()).toBe(1);
    expect(steps).toEqual([{ kind: "done" }]);
  });

  it("asks again while the agent says it is too soon, reporting the wait", async () => {
    const fake = refusingFor(2);
    const steps: EndStep[] = [];
    await endMaintenance(
      fake.bridge,
      (step) => steps.push(step),
      async () => {},
    );
    expect(fake.calls()).toBe(3);
    expect(steps).toEqual([{ kind: "waiting", seconds: 54 }, { kind: "waiting", seconds: 53 }, { kind: "done" }]);
  });

  it("gives up on any other error", async () => {
    const bridge = {
      browser: {
        ...refusingFor(0).bridge.browser,
        maintenance: async () => {
          throw new Error("maintenance takes true or false, and a URL");
        },
      },
    };
    await expect(
      endMaintenance(
        bridge,
        () => {},
        async () => {},
      ),
    ).rejects.toThrow(/takes true or false/);
  });

  it("needs actions mode", async () => {
    await expect(
      endMaintenance(
        {},
        () => {},
        async () => {},
      ),
    ).rejects.toThrow(/actions/);
  });
});
