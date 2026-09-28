// Putting an image on the device from the browser, as client/src/update.rs
// `send` and `wait_back` do: hash it, begin, upload in acknowledged pieces
// from the device's offset (sending it again resumes), wait for the device
// to check and stage it, commit, and follow the reboot.

import { createSHA256 } from "hash-wasm";

import { answer, CHUNK, client, failure, type Schemas } from "../api/client";
import { clock, mb, Rate, stepLine } from "../describe/transfer";
import { COME_BACK } from "../describe/update";
import { Line } from "../text/line";
import { putPiece } from "./raw";
import { pause, stop, type Report } from "./work";

export interface Plan {
  image: File;
  bmap: File;
  /** Re-create /data: every setting, the claim, the browser profile and the identity go. */
  wipeData: boolean;
  /** Write the whole disk: partition table, boot, root and /data. */
  repartition: boolean;
  /** Have the device check the whole upload against its SHA-256. */
  verify: boolean;
  /** Reboot into it once it is committed. */
  reboot: boolean;
}

export type Sent = "staged" | "rebooting" | "wiped";

function wipes(plan: Plan): boolean {
  return plan.wipeData || plan.repartition;
}

/** Hash, upload, have the device check and stage it, commit it. */
export async function send(plan: Plan, node: string, report: Report): Promise<Sent> {
  const bmap = await plan.bmap.text();
  const name = plan.image.name;
  const size = plan.image.size;

  const sha256 = await hash(plan.image, report);
  stop(report);
  const begun = await answer(
    client.POST("/api/v1/update/begin", {
      body: { name, size, sha256, bmap, verify: plan.verify, repartition: plan.repartition },
    }),
  );
  if (begun.phase === "receiving") {
    await upload(plan.image, begun.offset, report);
  } else {
    report.line(Line.of("ok", `${node} already has ${name}`));
  }

  await prepare(report);
  stop(report);
  const done = await answer(
    client.POST("/api/v1/update/commit", { body: { wipe_data: wipes(plan), reboot: plan.reboot } }),
  );
  report.line(Line.of("ok", done.message));

  if (!plan.reboot) {
    report.line(
      Line.of("cmd", "`tessaro-ctl device reboot`")
        .text(" applies it; ")
        .add("cmd", "`tessaro-ctl update cancel`")
        .text(" drops it"),
    );
    return wipes(plan) ? "wiped" : "staged";
  }
  report.line(Line.of("warn", `${node} is rebooting to apply it - this takes a few minutes; do not power it off`));
  if (wipes(plan)) {
    report.line(Line.of("warn", `${node} comes back unclaimed, with a new name and certificate`));
    return "wiped";
  }
  return "rebooting";
}

async function hash(image: File, report: Report): Promise<string> {
  const hasher = await createSHA256();
  hasher.init();
  const size = image.size;
  for (let done = 0; done < size;) {
    stop(report);
    const piece = new Uint8Array(await image.slice(done, Math.min(size, done + CHUNK)).arrayBuffer());
    hasher.update(piece);
    done += piece.length;
    report.progress(stepLine("label", "hashing", `${mb(done)}/${mb(size)}`), done, size);
  }
  report.line(stepLine("ok", "hashed", mb(size)));
  return hasher.digest("hex");
}

async function upload(image: File, from: number, report: Report): Promise<void> {
  const size = image.size;
  const resumed = from > 0 ? Line.of("muted", `  (resumed at ${mb(from)})`) : new Line();
  const rate = new Rate(from);
  let offset = from;
  while (offset < size) {
    stop(report);
    const piece = image.slice(offset, Math.min(size, offset + CHUNK));
    let received: Schemas["Received"];
    try {
      received = await putPiece("/api/v1/update/image", { offset }, piece, report.signal);
    } catch (error) {
      throw new Error(`the upload stopped at ${mb(offset)}: ${failure(error).message}; send it again to resume`);
    }
    offset = received.received;
    report.progress(stepLine("label", "uploading", rate.line(offset, size).join(resumed)), offset, size);
  }
  report.line(stepLine("ok", "uploaded", `${mb(size - from)} in ${clock(rate.elapsed())}`));
}

/** Poll until the device has verified and staged the image. */
async function prepare(report: Report): Promise<void> {
  // The step being shown, and its rate since it started.
  let step = null as { phase: Schemas["UpdatePhase"]; rate: Rate } | null;
  for (;;) {
    stop(report);
    const status = await answer(client.GET("/api/v1/update"));
    switch (status.phase) {
      case "verifying":
      case "preparing": {
        const [label, done, total] =
          status.phase === "verifying"
            ? ["verifying", status.verified, status.size]
            : ["preparing", status.prepared, status.to_prepare];
        if (step?.phase !== status.phase) {
          if (step?.phase === "verifying") {
            report.line(stepLine("ok", "verified", `${mb(status.size)} in ${clock(step.rate.elapsed())}`));
          }
          step = { phase: status.phase, rate: new Rate(done) };
        }
        if (total > 0) {
          report.progress(stepLine("label", label, step.rate.line(done, total)), done, total);
        } else {
          report.progress(stepLine("label", label, "starting"), 0, 1);
        }
        await pause(report, 1000);
        break;
      }
      case "ready":
      case "pending":
        report.line(stepLine("ok", "prepared", `${mb(status.to_prepare)} of the image checked`));
        return;
      case "failed":
        throw new Error(`the device refused the image: ${status.error ?? "no reason given"}`);
      case "idle":
      case "receiving":
        throw new Error("the device lost the upload; send it again");
    }
  }
}

/**
 * After "rebooting": ask until the device answers again, then say what the
 * apply did. Its identity answers without a credential; how the update went
 * needs this browser's session, which a reboot ends on a claimed device.
 */
export async function waitBack(node: string, report: Report): Promise<string> {
  const started = performance.now();
  const elapsed = () => clock((performance.now() - started) / 1000);
  // It takes a moment to go down; answering straight away is the old boot.
  await pause(report, 10_000);
  for (;;) {
    report.progress(stepLine("label", "waiting", `for ${node} to come back (${elapsed()})`), 0, 1);
    try {
      await answer(client.GET("/api/v1/device/id"));
      break;
    } catch {
      if ((performance.now() - started) / 1000 > COME_BACK) {
        throw new Error(
          `${node} has not answered for ${elapsed()}; it may still be writing - watch its console, or look at this page later`,
        );
      }
      await pause(report, 5000);
    }
  }
  report.line(Line.of("ok", `${node} is back after ${elapsed()}`));
  let update: Schemas["UpdateStatus"];
  let status: Schemas["Status"];
  try {
    update = await answer(client.GET("/api/v1/update"));
    status = await answer(client.GET("/api/v1/device/status"));
  } catch (error) {
    if (failure(error).signedOut) {
      return "back; sign in again to see how the update went";
    }
    throw error;
  }
  const last = update.last;
  if (!last) {
    throw new Error(`${node} is back but reports no update result; the Update page has more`);
  }
  if (!last.applied) {
    throw new Error(`the update was not applied: ${last.message}`);
  }
  report.line(Line.of("ok", last.message));
  if (status.os) {
    report.line(
      Line.of("ok", `${node} now runs ${status.os}${status.image_version ? `, image ${status.image_version}` : ""}`),
    );
  }
  return "applied";
}
