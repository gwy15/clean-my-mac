import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { JSDOM } from "jsdom";
import { mountApp } from "../ui/app.js";

const html = await readFile(new URL("../ui/index.html", import.meta.url), "utf8");
const gib = 1024 ** 3;
const snapshot = {
  items: [
    { id: "cache", name: "缓存 <script>", path: "~/Library/Caches/Test", description: "可重建缓存", kind: "可选缓存", allocatedBytes: gib, reclaimableBytes: gib, selectable: true, status: "可清理", errors: [] },
    { id: "blocked", name: "受限目录", path: "~/Library/Test", description: "无权限", kind: "可选缓存", allocatedBytes: 0, reclaimableBytes: 0, selectable: false, status: "无法读取", errors: ["Permission denied"] },
  ],
  volume: { usedBytes: 100 * gib, availableBytes: 200 * gib },
  fullDiskAccess: "未授权",
  volumeError: null,
};

function setup(invoke, permission = async () => snapshot.fullDiskAccess) {
  const dom = new JSDOM(html);
  const document = dom.window.document;
  const dialog = document.getElementById("confirmation");
  dialog.showModal = () => dialog.setAttribute("open", "");
  dialog.close = () => {
    dialog.removeAttribute("open");
    dialog.dispatchEvent(new dom.window.Event("close"));
  };
  const app = mountApp(document, (command, args) => command === "disk_access" ? permission() : invoke(command, args), () => ({}));
  const $ = (id) => document.getElementById(id);
  const select = () => $("select-cache").click();
  return { document, app, $, select };
}

async function settle() {
  await new Promise((resolve) => setImmediate(resolve));
}

test("scan is read-only, starts unselected and disables unreadable items", async () => {
  const calls = [];
  const { app, $, document } = setup(async (command) => { calls.push(command); return snapshot; });
  await app.scan();
  assert.deepEqual(calls, ["scan"]);
  assert.equal($("select-cache").checked, false);
  assert.equal($("select-blocked").disabled, true);
  assert.equal($("review").disabled, true);
  assert.equal($("available").textContent, "200.0 GiB");
  assert.equal($("reclaimable").textContent, "1.000 GiB");
  assert.equal(document.querySelectorAll(".item script").length, 0);
  assert.match($("items").textContent, /缓存 <script>/);
});

test("cancel never deletes; confirmation sends only selected IDs and refreshes", async () => {
  const calls = [];
  const { app, $, select } = setup(async (command, args) => {
    calls.push([command, args]);
    return command === "clean" ? { reclaimedBytes: gib, failures: [] } : snapshot;
  });
  await app.scan();
  select();
  $("review").click();
  assert.equal($("confirmation").open, true);
  assert.match($("confirm-items").textContent, /缓存/);
  assert.equal($("cancel"), $("cancel").ownerDocument.activeElement);
  $("cancel").click();
  assert.equal(calls.length, 1);
  $("review").click();
  $("confirm").click();
  $("confirm").click();
  assert.equal($("rescan").disabled, true);
  await settle();
  assert.deepEqual(calls.map(([command]) => command), ["scan", "clean", "scan"]);
  assert.deepEqual(calls[1][1], { ids: ["cache"], confirmed: true });
  assert.equal($("review").disabled, true);
  assert.match($("status").textContent, /实际释放约 1.000 GiB/);
});

test("partial cleanup errors remain visible after refresh", async () => {
  const { app, $, select } = setup(async (command) => command === "clean"
    ? { reclaimedBytes: 0, failures: ["缓存正在使用"] } : snapshot);
  await app.scan();
  select();
  $("review").click();
  $("confirm").click();
  await settle();
  assert.match($("status").textContent, /缓存正在使用/);
  assert.equal($("status").classList.contains("error"), true);
});

test("failed refresh clears stale data and allows retry", async () => {
  let scans = 0;
  const { app, $, select } = setup(async (command) => {
    if (command === "clean") throw new Error("cleanup failed");
    if (++scans === 2) throw new Error("scan failed");
    return snapshot;
  });
  await app.scan();
  select();
  $("review").click();
  $("confirm").click();
  await settle();
  assert.equal($("items").children.length, 0);
  assert.equal($("available").textContent, "—");
  assert.equal($("review").disabled, true);
  assert.equal($("rescan").disabled, false);
  assert.match($("status").textContent, /cleanup failed/);
  assert.match($("status").textContent, /scan failed/);
  await app.scan();
  assert.equal($("items").children.length, 2);
});

test("scan failure and settings failure are surfaced", async () => {
  const { app, $ } = setup(async () => { throw new Error("unavailable"); });
  await app.scan();
  assert.match($("status").textContent, /扫描失败/);
  assert.equal($("review").disabled, true);
  $("settings").click();
  await settle();
  assert.match($("status").textContent, /无法打开权限设置/);
});

const pendingItems = snapshot.items.map((item) => ({
  ...item, selectable: false, allocatedBytes: 0, reclaimableBytes: 0, status: "待扫描", errors: [],
}));

test("permission and individual rows update before scan finishes; details stay open", async () => {
  let finish;
  let channel;
  const { app, $, document } = setup((_command, args) => {
    channel = args.onProgress;
    channel.onmessage({ type: "started", items: structuredClone(pendingItems) });
    return new Promise((resolve) => { finish = resolve; });
  });
  const running = app.scan();
  await settle();
  assert.equal($("permission").textContent, "完全磁盘访问：未授权");
  assert.equal($("settings").disabled, false);
  assert.equal($("items").children.length, 2);
  const row = $("row-cache");
  assert.equal(row.querySelector(".size").textContent, "—");
  const toggle = row.querySelector("button");
  toggle.click();
  toggle.focus();
  channel.onmessage({ type: "scanning", id: "cache", completed: 0, total: 2 });
  assert.match($("status").textContent, /0\/2.*缓存/);
  assert.equal(row.dataset.status, "scanning");
  channel.onmessage({ type: "volume", volume: snapshot.volume, error: null });
  assert.equal($("available").textContent, "200.0 GiB");
  channel.onmessage({ type: "item", item: snapshot.items[0], completed: 1, total: 2 });
  assert.match($("reclaimable").textContent, /1.000 GiB.*已发现/);
  assert.equal($("row-cache"), row);
  assert.equal($("details-cache").hidden, false);
  assert.equal(document.activeElement, toggle);
  assert.equal($("select-cache").disabled, true);
  assert.equal($("review").disabled, true);
  finish(snapshot);
  await running;
  assert.equal($("select-cache").disabled, false);
  assert.equal($("details-cache").hidden, false);
  assert.equal($("reclaimable").textContent, "1.000 GiB");
});

test("failed partial scans cannot clean; late events cannot overwrite a retry", async () => {
  let calls = 0;
  let lateEvent;
  const { app, $ } = setup(async (_command, args) => {
    if (++calls > 1) return snapshot;
    lateEvent = args.onProgress.onmessage;
    lateEvent({ type: "started", items: structuredClone(pendingItems) });
    lateEvent({ type: "item", item: snapshot.items[0], completed: 1, total: 2 });
    throw new Error("interrupted");
  });
  await app.scan();
  assert.equal($("items").children.length, 2);
  assert.equal($("select-cache").disabled, true);
  assert.equal($("review").disabled, true);
  assert.equal($("permission").textContent, "完全磁盘访问：未授权");
  assert.match($("status").textContent, /interrupted/);
  await app.scan();
  lateEvent({ type: "started", items: [] });
  assert.equal($("items").children.length, 2);
  assert.equal($("select-cache").disabled, false);
});

test("a slow or failed permission probe does not delay scan results", async () => {
  let failPermission;
  const { app, $ } = setup(async () => snapshot, () => new Promise((_resolve, reject) => { failPermission = reject; }));
  await app.scan();
  assert.equal($("items").children.length, 2);
  assert.match($("permission").textContent, /检查中/);
  failPermission(new Error("probe failed"));
  await settle();
  assert.match($("permission").textContent, /probe failed/);
  assert.equal($("select-cache").disabled, false);
});
