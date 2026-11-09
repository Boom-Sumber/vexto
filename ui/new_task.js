// 新建任务窗口（按 Rayburst 参考界面还原）。
//
// 两个页签：「链接任务」（http/https/ftp/magnet）与「种子任务」（.torrent）。
// 存储路径 / 可用空间 / 高级选项是**共享区**，两个页签都能改；仅链接任务
// 有意义的字段（UA / Referer / 校验和 / HTTP 鉴权）在种子页签隐藏。
// 单链接时自动探测文件名与大小；多链接时名字框留空、只提示「选填」，
// 「重新获取」禁用——一个名字框表达不了多个不同文件名。

const T = window.__TAURI__;
const invoke = T.core.invoke;
const listen = T.event.listen;
const el = (id) => document.getElementById(id);

// 浏览器扩展交过来的附加信息：url → { filename, size, referer, userAgent, cookie }。
// 热键预填只有 url，不会有这些字段。
const extras = new Map();

let defaultDir = "";
let submitting = false;
let probeSeq = 0;
let probeTimer = null;
let lastSize = 0;

// 当前页签与已读入的种子文件（base64 / 原始字节数 / 文件名）。
let activeTab = "link";
let torrentB64 = "";
let torrentBytes = 0;
let torrentName = "";

// 与后端 `validate::MAX_TORRENT_B64` 对应的原文件上限（约 4 MB）。
const MAX_TORRENT_FILE = 4 * 1024 * 1024;

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

function showError(msg, which) {
  const box = el(which === "torrent" ? "tError" : "error");
  box.textContent = msg || "";
  box.classList.toggle("hidden", !msg);
}

function clearErrors() {
  showError("");
  showError("", "torrent");
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
  activeTab = link ? "link" : "torrent";
  el("tabLink").classList.toggle("active", link);
  el("tabTorrent").classList.toggle("active", !link);
  el("paneLink").classList.toggle("hidden", !link);
  el("paneTorrent").classList.toggle("hidden", link);
  // 校验和 / HTTP 鉴权 / UA / Referer 对 BT 没有意义，切到种子页签就藏起来，
  // 免得用户填了却不生效（种子里的分片校验由 torrent 自己带）。
  for (const row of document.querySelectorAll("[data-link-only]")) {
    row.classList.toggle("hidden", !link);
  }
  clearErrors();
  updateSpaceHint();
}

// ── 重命名 / 大小 / 可用空间 ────────────────────────────────────────────

/// 多链接时名字框清空并只显示「选填」提示，「重新获取」禁用。
function syncNameState() {
  const n = splitLinks().length;
  const multi = n > 1;
  el("reprobe").disabled = multi || n === 0;
  el("name").placeholder = "选填";
  // 校验和对多条链接不适用（每个文件内容不同），UI 上直接禁掉，
  // 免得用户填了却在提交时被静默丢弃。
  el("checksum").disabled = multi;
  el("checksum").title = multi ? "多条链接不能共用一个校验和" : "";
  if (multi) {
    el("name").value = "";
    el("nameSize").textContent = "";
    el("checksum").value = "";
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
  if (activeTab === "torrent") {
    // 种子文件本身的大小 ≠ 任务总大小（要解析 bencode 才知道），
    // 所以只如实写「种子 x KB」，不冒充「需下载 x」。
    if (torrentBytes) parts.push("种子 " + fmtBytes(torrentBytes));
  } else if (lastSize) {
    parts.push("需下载 " + fmtBytes(lastSize));
  }
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

// ── 种子文件 ────────────────────────────────────────────────────────────

/// Uint8Array → base64。分块避免 `apply` 一次塞太多参数导致栈溢出。
function bytesToBase64(bytes) {
  let s = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    s += String.fromCharCode.apply(null, bytes.subarray(i, i + CHUNK));
  }
  return btoa(s);
}

function resetTorrent() {
  torrentB64 = "";
  torrentBytes = 0;
  torrentName = "";
  el("torrentName").value = "";
  el("torrentSize").textContent = "";
  updateSpaceHint();
}

/// 只做两道轻量校验：大小上限 + bencode 顶层必须是 `d…e` 字典。
/// 真正的种子解析交给 aria2（它会给出明确的 RPC 错误），前端不引 bencode 库。
async function onTorrentPicked(ev) {
  const file = ev.target.files && ev.target.files[0];
  if (!file) return;
  // 允许连续选同一个文件（否则第二次 change 不触发）
  ev.target.value = "";
  showError("", "torrent");

  if (file.size > MAX_TORRENT_FILE) {
    resetTorrent();
    showError("种子文件过大（上限 4 MB）", "torrent");
    return;
  }
  let bytes;
  try {
    bytes = new Uint8Array(await file.arrayBuffer());
  } catch (e) {
    resetTorrent();
    showError("读取种子文件失败：" + errText(e), "torrent");
    return;
  }
  if (bytes.length < 2 || bytes[0] !== 0x64 || bytes[bytes.length - 1] !== 0x65) {
    resetTorrent();
    showError("这不像是 .torrent 文件（内容应为 bencode 字典 d…e）", "torrent");
    return;
  }

  torrentB64 = bytesToBase64(bytes);
  torrentBytes = bytes.length;
  torrentName = file.name;
  el("torrentName").value = file.name;
  el("torrentSize").textContent = fmtBytes(bytes.length);
  updateSpaceHint();
}

// ── 提交 ────────────────────────────────────────────────────────────────

async function submit() {
  if (submitting) return;
  return activeTab === "torrent" ? submitTorrent() : submitLinks();
}

async function submitLinks() {
  const links = splitLinks();
  if (!links.length) {
    showError("至少填一个下载链接");
    return;
  }

  const single = links.length === 1;
  const base = {
    dir: el("dir").value.trim() || defaultDir,
    max_speed_kb: Number(el("speed").value) || 0,
    connections: Number(el("conn").value) || 0,
    user_agent: el("ua").value.trim(),
    referer: el("referer").value.trim(),
    // 校验和只对单条链接有意义：多条链接内容是不同文件，不可能共用一个摘要。
    // 后端 `validate::checksum_option` 会再校验格式，这里只在多链接时丢弃。
    checksum: single ? el("checksum").value.trim() : "",
    http_user: el("httpUser").value.trim(),
    http_passwd: el("httpPasswd").value,
  };
  const typedName = single ? el("name").value.trim() : "";
  const tasks = links.map((uri) => {
    const ex = extras.get(uri) || {};
    return {
      uri,
      dir: base.dir,
      max_speed_kb: base.max_speed_kb,
      connections: base.connections,
      // 优先级：用户在框里改的 > 扩展给的原文件名 > 空（后端从 URL 推）
      name: typedName || ex.filename || "",
      // UA / Referer 以用户手填为准；没填则用扩展带来的（防盗链/登录态需要）
      user_agent: base.user_agent || ex.userAgent || "",
      referer: base.referer || ex.referer || "",
      // Cookie 不上界面，直接随任务下发（扩展抢来的登录态）
      cookie: ex.cookie || "",
      checksum: base.checksum,
      http_user: base.http_user,
      http_passwd: base.http_passwd,
    };
  });

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

async function submitTorrent() {
  if (!torrentB64) {
    showError("请先选择一个 .torrent 文件", "torrent");
    return;
  }
  const task = {
    torrent_base64: torrentB64,
    // 名字只用于设置页状态行回显；aria2 用种子里的 info.name，不接受覆盖。
    name: torrentName,
    dir: el("dir").value.trim() || defaultDir,
    max_speed_kb: Number(el("speed").value) || 0,
  };

  submitting = true;
  el("submit").disabled = true;
  el("status").textContent = "正在创建…";
  showError("", "torrent");
  try {
    await invoke("download_add_torrent", { task });
    el("status").textContent = "已创建 1 个种子任务";
    invoke("new_task_done").catch(() => {});
  } catch (e) {
    showError("创建种子任务失败：" + errText(e), "torrent");
    el("status").textContent = "";
  } finally {
    submitting = false;
    el("submit").disabled = false;
  }
}

// ── 初始化 ──────────────────────────────────────────────────────────────

/// 追加预填条目。热键只给一条纯 URL；浏览器扩展会给 url + 文件名/大小/cookie。
/// 窗口已开着时**追加**到输入框末尾，不覆盖用户已经改过的内容。
function addPrefill(payload) {
  const list = Array.isArray(payload) ? payload : payload ? [payload] : [];
  if (!list.length) return;

  const known = new Set(splitLinks());
  const fresh = [];
  for (const raw of list) {
    const item = typeof raw === "string" ? { url: raw } : raw || {};
    const url = String(item.url || "").trim();
    if (!url || known.has(url)) continue;
    known.add(url);
    fresh.push(url);
    extras.set(url, {
      filename: String(item.filename || "").trim(),
      size: Number(item.size) || 0,
      referer: String(item.referer || "").trim(),
      userAgent: String(item.user_agent || item.userAgent || "").trim(),
      cookie: String(item.cookie || ""),
    });
  }
  if (!fresh.length) return;

  // 热键 / 扩展送来的都是链接，别让它们填进一个当前不可见的输入框。
  setTab("link");

  const cur = el("links").value.trim();
  el("links").value = cur ? cur + "\n" + fresh.join("\n") : fresh.join("\n");

  const links = splitLinks();
  const ex = links.length === 1 ? extras.get(links[0]) || {} : null;
  el("name").dataset.userEdited = "";
  if (ex && ex.filename) {
    el("name").value = ex.filename;
    // 别让自动探测把扩展给的原文件名覆盖掉；用户仍可点「重新获取」。
    el("name").dataset.userEdited = "1";
    el("nameSize").textContent = ex.size ? fmtBytes(ex.size) : "";
  }
  syncNameState();

  // 扩展给了大小时不再探测：探测失败会把刚显示的大小清空。
  if (ex && ex.size) updateSpaceHint(ex.size);
  else scheduleProbe();

  el("links").focus();
  const v = el("links").value;
  el("links").setSelectionRange(v.length, v.length);
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

  // 原生 file input 藏在 .hidden 里，只由自绘按钮触发（系统部件是白底）。
  el("pickTorrent").addEventListener("click", () => el("torrentFile").click());
  el("torrentFile").addEventListener("change", onTorrentPicked);

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
    // 事件只是通知：数据统一由 take_new_task_prefill 取走，避免窗口刚建、
    // 监听还没注册时事件丢失（连续两个下载会一起丢）。
    await listen("new-task-prefill", async () => {
      try {
        addPrefill(await invoke("take_new_task_prefill"));
      } catch (e) {
        /* 取不到不是错误 */
      }
    });
  } catch (e) {
    showError("订阅预填事件失败：" + errText(e));
  }

  try {
    addPrefill(await invoke("take_new_task_prefill"));
  } catch (e) {
    /* 没有预填不是错误 */
  }

  syncNameState();
  updateSpaceHint();
  el("links").focus();
})();
