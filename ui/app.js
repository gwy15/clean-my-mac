export function formatBytes(bytes) {
  const value = bytes / 1024 ** 3;
  return `${value.toFixed(value >= 1000 ? 0 : value >= 100 ? 1 : value >= 10 ? 2 : 3)} GiB`;
}

export function mountApp(document, invoke, createChannel) {
  const $ = (id) => document.getElementById(id);
  let items = [];
  let selected = new Set();
  let busy = false;
  let pendingIds = [];
  let permissionRequest = 0;

  function message(text, error = false) {
    $("status").textContent = text;
    $("status").classList.toggle("error", error);
  }

  function chosen() {
    return items.filter((item) => selected.has(item.id) && item.selectable);
  }

  function total(list) {
    return list.reduce((sum, item) => sum + item.reclaimableBytes, 0);
  }

  function updateControls() {
    const list = chosen();
    $("selection").textContent = list.length
      ? `已选 ${list.length} 项 · 预计释放 ${formatBytes(total(list))}`
      : "未选择项目";
    $("review").disabled = busy || list.length === 0;
    $("rescan").disabled = busy;
    $("confirm").disabled = busy;
    document.querySelector(".candidates").setAttribute("aria-busy", String(busy));
    for (const input of $("items").querySelectorAll("input")) {
      input.checked = selected.has(input.dataset.id);
      input.disabled = busy || !items.find((item) => item.id === input.dataset.id)?.selectable;
    }
  }

  function element(tag, text, className) {
    const node = document.createElement(tag);
    node.textContent = text;
    if (className) node.className = className;
    return node;
  }

  function renderVolume(volume) {
    $("used").textContent = volume ? formatBytes(volume.usedBytes) : "—";
    $("available").textContent = volume ? formatBytes(volume.availableBytes) : "—";
  }

  function updateItem(row, item) {
    const statusText = item.selectable ? item.kind : item.status;
    row.dataset.status = {
      "建议清理": "recommended", "可选缓存": "optional", "谨慎清理": "caution",
      "无可清理内容": "empty", "无法读取": "error", "正在使用": "busy",
      "受保护": "protected", "待扫描": "pending", "扫描中": "scanning", "扫描中断": "error",
    }[statusText] ?? "neutral";
    row.querySelector(".item-status").textContent = statusText;
    const pending = ["待扫描", "扫描中", "扫描中断"].includes(item.status);
    const size = row.querySelector(".size");
    size.textContent = pending ? "—" : formatBytes(item.reclaimableBytes);
    size.title = pending ? "尚未完成统计" : `已占用 ${formatBytes(item.allocatedBytes)}`;
    const errors = row.querySelector(".item-errors");
    errors.textContent = item.errors.join("\n");
    errors.hidden = !item.errors.length;
    row.querySelector("input").disabled = busy || !item.selectable;
  }

  function renderItems(nextItems) {
    items = nextItems;
    $("items").replaceChildren();
    for (const item of items) {
      const row = element("div", "", "item");
      row.id = `row-${item.id}`;
      const input = document.createElement("input");
      input.type = "checkbox";
      input.id = `select-${item.id}`;
      input.dataset.id = item.id;
      input.disabled = !item.selectable;
      input.addEventListener("change", () => {
        if (input.checked) selected.add(item.id);
        else selected.delete(item.id);
        updateControls();
      });
      const label = element("label", item.name);
      label.htmlFor = input.id;
      label.title = item.name;
      const status = element("span", "", "item-status");
      const content = element("div", "", "item-details");
      content.id = `details-${item.id}`;
      content.hidden = true;
      content.append(element("p", item.description, "description"), element("p", item.path, "path"));
      content.append(element("p", "", "item-errors"));
      const toggle = element("button", "详情", "details-toggle");
      toggle.type = "button";
      toggle.setAttribute("aria-controls", content.id);
      toggle.setAttribute("aria-expanded", "false");
      toggle.setAttribute("aria-label", `${item.name}的详情`);
      toggle.addEventListener("click", () => {
        content.hidden = !content.hidden;
        toggle.setAttribute("aria-expanded", String(!content.hidden));
        toggle.textContent = content.hidden ? "详情" : "收起";
      });
      const size = element("div", "", "size");
      row.append(input, label, status, size, toggle, content);
      updateItem(row, item);
      $("items").append(row);
    }
  }

  function replaceItem(item) {
    const index = items.findIndex((entry) => entry.id === item.id);
    if (index === -1) return;
    items[index] = item;
    updateItem($(`row-${item.id}`), item);
  }

  async function refreshPermission() {
    const request = ++permissionRequest;
    $("permission").textContent = "完全磁盘访问：检查中";
    try {
      const access = await invoke("disk_access");
      if (request !== permissionRequest) return;
      $("permission").textContent = `完全磁盘访问：${access}`;
      $("permission-hint").hidden = access === "已授权";
    } catch (error) {
      if (request !== permissionRequest) return;
      $("permission").textContent = `完全磁盘访问：检查失败（${error}）`;
      $("permission-hint").hidden = false;
    }
  }

  async function scanFiles(prefix = "", prefixFailed = false) {
    selected.clear();
    renderItems([]);
    renderVolume(null);
    $("reclaimable").textContent = "—";
    updateControls();
    void refreshPermission();
    const channel = createChannel();
    let active = true;
    const progressMessage = (text) => message(`${prefix}${prefix ? "\n" : ""}${text}`, prefixFailed);
    channel.onmessage = (event) => {
      if (!active) return;
      if (event.type === "started") renderItems(event.items);
      if (event.type === "volume") renderVolume(event.volume);
      if (event.type === "scanning") {
        const item = items.find((entry) => entry.id === event.id);
        if (item) {
          replaceItem({ ...item, status: "扫描中" });
          progressMessage(`已扫描 ${event.completed}/${event.total} 项 · 正在扫描 ${item.name}`);
        }
      }
      if (event.type === "item") {
        replaceItem(event.item);
        $("reclaimable").textContent = `${formatBytes(total(items.filter((item) => item.selectable)))}（已发现）`;
        progressMessage(`已扫描 ${event.completed}/${event.total} 项`);
      }
    };
    progressMessage("正在准备扫描…");
    try {
      const snapshot = await invoke("scan", { onProgress: channel });
      active = false;
      if (!items.length) renderItems(snapshot.items);
      else for (const item of snapshot.items) replaceItem(item);
      renderVolume(snapshot.volume);
      $("reclaimable").textContent = formatBytes(total(items.filter((item) => item.selectable)));
      message(`${prefix || "扫描完成。请选择需要清理的项目。"}${snapshot.volumeError ? `\n磁盘容量读取失败：${snapshot.volumeError}` : ""}`, prefixFailed || Boolean(snapshot.volumeError));
    } catch (error) {
      // Partial results remain readable, but a failed scan cannot authorize any cleanup.
      for (const item of [...items]) {
        replaceItem({ ...item, selectable: false, status: ["待扫描", "扫描中"].includes(item.status) ? "扫描中断" : item.status });
      }
      $("reclaimable").textContent = "—";
      progressMessage(`${prefix ? "刷新" : "扫描"}失败：${error}。请重新扫描。`);
      $("status").classList.add("error");
    } finally {
      active = false;
      channel.onmessage = () => {};
    }
  }

  async function scan() {
    if (busy) return;
    busy = true;
    try {
      await scanFiles();
    } finally {
      busy = false;
      updateControls();
    }
  }

  $("rescan").addEventListener("click", scan);
  $("settings").addEventListener("click", async () => {
    try {
      await invoke("open_settings");
      message("已打开权限设置。请授权 Clean My Mac，然后退出并重新打开 App。");
    } catch (error) {
      message(`无法打开权限设置：${error}`, true);
    }
  });
  $("review").addEventListener("click", () => {
    if (busy || !chosen().length) return;
    const list = chosen();
    pendingIds = list.map((item) => item.id);
    $("confirm-items").replaceChildren(...list.map((item) => {
      const entry = element("li", `${item.name} · ${formatBytes(item.reclaimableBytes)}`);
      entry.append(element("p", item.description));
      return entry;
    }));
    $("confirm-total").textContent = `预计释放 ${formatBytes(total(list))}`;
    $("confirmation").showModal();
    $("cancel").focus();
  });
  $("cancel").addEventListener("click", () => $("confirmation").close());
  $("confirmation").addEventListener("close", () => { pendingIds = []; });
  $("confirm").addEventListener("click", async () => {
    if (busy || !pendingIds.length) return;
    const ids = [...pendingIds];
    pendingIds = [];
    $("confirmation").close();
    busy = true;
    selected.clear();
    updateControls();
    message("正在清理，请勿关闭 App…");
    let resultMessage;
    let failed = false;
    try {
      const report = await invoke("clean", { ids, confirmed: true });
      resultMessage = `清理完成，实际释放约 ${formatBytes(report.reclaimedBytes)}。`;
      if (report.failures.length) {
        failed = true;
        resultMessage += `\n${report.failures.length} 项清理失败：\n${report.failures.join("\n")}`;
      }
    } catch (error) {
      failed = true;
      resultMessage = `清理失败：${error}`;
    }
    try {
      await scanFiles(resultMessage, failed);
    } finally {
      busy = false;
      updateControls();
    }
  });
  return { scan };
}

if (typeof window !== "undefined" && window.__TAURI__) {
  const app = mountApp(window.document, window.__TAURI__.core.invoke, () => new window.__TAURI__.core.Channel());
  app.scan();
} else if (typeof window !== "undefined") {
  document.getElementById("status").textContent = "请通过桌面 App 打开此界面。";
  for (const button of document.querySelectorAll("button")) button.disabled = true;
}
