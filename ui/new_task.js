// 新建任务窗口（按 Rayburst 参考界面还原）。
//
// 两个页签：「链接任务」可用，「种子任务」暂未支持（只做占位，不假装能用）。
// 单链接时自动探测文件名与大小；多链接时名字框留空、只提示「选填」，
// 「重新获取」禁用——一个名字框表达不了多个不同文件名。

const T = window.__TAURI__;
const invoke = T.core.invoke;
const listen = T.event.listen;
const el = (id) => document.getElementById(id);

let defaultDir = "";
let submitting = false;
let probeSeq = 0;
let probeTimer = null;
let lastSize = 0;

// ── 小工具 ──────────────────────────────────────────────────────────────

function fmtBytes(n) {
  n = Number(n) || 0;
  if (!n) return "";
  if (n < 1024) return n + " B";
  const u = ["KB", "MB", "GB", "TB"];
  let i = -1;
  do { n /= 1024; i++; } while (n >= 1024 && i < u.length - 1);
  // 统一 1 位小数：153.3 GB / 1.3 KB / 12 MB（去掉无意义的 .0）
  return n.toFixed(1).replace(/\.0$/, "") + " " + u[i];
}

function errText(e) {
  if (e == null) return "未知错误";
  if (typeof e === "string") return e;
  if (e.message) return String(e.message);
  return String(e);
}

function showError(msg) {
  const box = el("error");
  box.textContent = msg || "";
  box.classList.toggle("hidden", !msg);
}

function splitLinks() {
  return el("links").value
    .split(/\r?\n/)
    .map((s) => s.trim())
    .filter(Boolean);
}

/// 前端先按 URL 猜一个名字，别让用户盯着空框等网络。
function guessName(url) {
  try {
    const u = new URL(url);
    const seg = u.pathname.split("/").filter(Boolean).pop() || "";
    return seg ? decodeURIComponent(seg) : "";
  } catch (e) {
    return "";
  }
}

// ── 页签 ────────────────────────────────────────────────────────────────

function setTab(which) {
  const link = which === "link";
  el("tabLink").classList.toggle("active", link);
  el("tabTorrent").classList.toggle("active", !link);
  el("paneLink").classList.toggle("hidden", !link);
  el("paneTorrent").classList.toggle("hidden", link);
}

// ── 重命名 / 大小 / 可用空间 ────────────────────────────────────────────

/// 多链接时名字框清空并只显示「选填」提示，「重新获取」禁用。
function syncNameState() {
  const n = splitLinks().length;
  const multi = n > 1;
  el("reprobe").disabled = multi || n === 0;
  el("name").placeholder = "选填";
  if (multi) {
    el("name").value = "";
    el("nameSize").textContent = "";
    lastSize = 0;
  }
}

async function updateSpaceHint(size) {
  if (size !== undefined) lastSize = size;
  const dir = el("dir").value.trim() || defaultDir;
  let free = null;
  try {
    free = await invoke("disk_free", { dir });
  } catch (e) {
    free = null;
  }
  const parts = [];
  if (lastSize) parts.push("需下载 " + fmtBytes(lastSize));
  if (free) parts.push("可用 " + fmtBytes(free));
  el("spaceHint").textContent = parts.join("，");
}

/// 单链接时探测文件名与大小。`force` 表示用户点了「重新获取」。
async function probeNow(force) {
  const links = splitLinks();
  const nameEl = el("name");

  if (links.length !== 1) {
    lastSize = 0;
    el("nameSize").textContent = "";
    updateSpaceHint();
    return;
  }

  if (force) {
    nameEl.dataset.userEdited = "";
    nameEl.value = "";
  }
  if (!nameEl.value && nameEl.dataset.userEdited !== "1") {
    nameEl.value = guessName(links[0]);
  }

  const seq = ++probeSeq;
  el("nameSize").textContent = "";
  lastSize = 0;
  updateSpaceHint();

  try {
    const r = await invoke("download_probe", { url: links[0] });
    if (seq !== probeSeq) return;      // 已有更新的探测，丢掉旧结果
    if (r && r.name && nameEl.dataset.userEdited !== "1") {
      nameEl.value = r.name;
    }
    if (r && r.size) {
      el("nameSize").textContent = fmtBytes(r.size);
      updateSpaceHint(r.size);
    }
  } catch (e) {
    /* 探测失败不影响手填，也不弹错误横幅 */
  }
}

function scheduleProbe() {
  clearTimeout(probeTimer);
  probeTimer = setTimeout(() => probeNow(false), 450);
}

// ── 提交 ────────────────────────────────────────────────────────────────

async function submit() {
  if (submitting) return;
  const links = splitLinks();
  if (!links.length) {
    showError("至少填一个下载链接");
    return;
  }

  const base = {
    name: el("name").value.trim(),
    dir: el("dir").value.trim() || defaultDir,
    max_speed_kb: Number(el("speed").value) || 0,
    connections: Number(el("conn").value) || 0,
    user_agent: el("ua").value.trim(),
    referer: el("referer").value.trim(),
  };
  const tasks = links.map((uri) => ({ uri, ...base }));

  submitting = true;
  el("submit").disabled = true;
  el("status").textContent = "正在创建…";
  showError("");
  try {
    const r = await invoke("download_add", { tasks });
    const failed = Array.isArray(r.failed) ? r.failed : [];
    if (failed.length) {
      showError(failed.map((f) => f.uri + "：" + f.error).join("\n"));
      el("status").textContent = `成功 ${r.ok} 个，失败 ${failed.length} 个`;
      el("links").value = failed.map((f) => f.uri).join("\n");
      syncNameState();
      scheduleProbe();
    } else {
      el("status").textContent = `已创建 ${r.ok} 个任务`;
      // 这个命令会销毁本窗口，响应可能收不到：不 await，也别让 rejection 冒成错误
      invoke("new_task_done").catch(() => {});
    }
  } catch (e) {
    showError("创建任务失败：" + errText(e));
    el("status").textContent = "";
  } finally {
    submitting = false;
    el("submit").disabled = false;
  }
}

// ── 初始化 ──────────────────────────────────────────────────────────────

function fillPrefill(text) {
  if (!text) return;
  el("links").value = text;
  el("name").dataset.userEdited = "";
  syncNameState();
  el("links").focus();
  el("links").setSelectionRange(text.length, text.length);
  scheduleProbe();
}

(async function init() {
  try {
    defaultDir = await invoke("download_default_dir");
  } catch (e) {
    defaultDir = "";
  }
  // 存储路径直接写值（不是占位符）：用户要求打开就能看到具体路径
  el("dir").value = defaultDir;

  el("tabLink").addEventListener("click", () => setTab("link"));
  el("tabTorrent").addEventListener("click", () => setTab("torrent"));

  // 标题栏的最小化 / 关闭由 titlebar.js 统一接管（window_action），
  // 页面里不再绑窗口事件，也不写 SVG（docs/13-UI-DESIGN-SYSTEM.md §13.5）。

  // 自绘图标：data-icon → Icons
  for (const b of document.querySelectorAll("[data-icon]")) {
    window.Icons.fill(b, b.dataset.icon, 16);
  }

  // 数字输入：系统 spinner 在深色下是白的，换成自绘步进
  window.NumField.enhance(el("speed"));
  window.NumField.enhance(el("conn"));

  el("links").addEventListener("input", () => {
    el("name").dataset.userEdited = "";
    syncNameState();
    scheduleProbe();
  });
  el("name").addEventListener("input", () => {
    el("name").dataset.userEdited = "1";
  });
  el("reprobe").addEventListener("click", () => probeNow(true));

  el("pickDir").addEventListener("click", () => {
    invoke("open_path", { path: el("dir").value.trim() || defaultDir }).catch(() => {});
  });
  el("defaultDir").addEventListener("click", () => {
    el("dir").value = defaultDir;
    updateSpaceHint();
  });
  el("dir").addEventListener("change", () => updateSpaceHint());

  el("advToggle").addEventListener("change", () => {
    el("advPane").classList.toggle("hidden", !el("advToggle").checked);
  });

  el("cancel").addEventListener("click", () => {
    invoke("close_new_task").catch(() => {});
  });
  el("submit").addEventListener("click", submit);
  el("links").addEventListener("keydown", (ev) => {
    if (ev.key === "Enter" && (ev.ctrlKey || ev.metaKey)) {
      ev.preventDefault();
      submit();
    }
  });

  try {
    await listen("new-task-prefill", (ev) => fillPrefill(ev.payload));
  } catch (e) {
    showError("订阅预填事件失败：" + errText(e));
  }

  try {
    fillPrefill(await invoke("take_new_task_prefill"));
  } catch (e) {
    /* 没有预填不是错误 */
  }

  syncNameState();
  updateSpaceHint();
  el("links").focus();
})();
