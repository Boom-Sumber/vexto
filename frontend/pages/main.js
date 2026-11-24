// 下载主窗口：数据全来自后端 `tasks:snapshot` 事件与 `download_snapshot` 命令。
//
// §S3.7.2 的硬要求：**增量更新**。已存在的任务行按 data-gid 复用节点，
// 只改文本与进度条宽度；只有「新出现」和「已消失」的任务才增删节点。
// 每 500ms 重建一遍 DOM 会让按钮点不中、滚动位置也丢。
//
// 分类是纯前端过滤：后端快照始终是全部任务，切换分类只给行加 `.hidden-row`，
// 不重新请求、也不重建节点。

const T = window.__TAURI__;
const invoke = T.core.invoke;
const listen = T.event.listen;
const el = (id) => document.getElementById(id);

// 所有 data-icon 占位统一由图标库填充（§13.5：页面里不许写内联 <svg>）。
for (const n of document.querySelectorAll("[data-icon]")) {
  Icons.fill(n, n.dataset.icon, Number(n.dataset.iconSize) || 16);
}
Icons.fill(el("tfOpenDir"), "folder", 16);

// gid -> 行节点。重建窗口时为空，由 download_snapshot() 一次性填满。
const rows = new Map();
let lastError = "";
let filter = "all";
let lastSnap = { tasks: [], global: {} };

// 后端的两条界面开关（`download_ui_flags`）：要不要二次确认、删任务会不会连文件一起删。
// 默认值取「要确认、不删文件」——没读到后端时也走最保守的那条。
let uiFlags = { confirm: true, delete_file: false, delete_mode: "recycle" };

const TITLES = {
  all: "全部",
  running: "进行中",
  failed: "失败",
  complete: "已完成",
  deleted: "已删除",
  github: "GitHub 加速",
};

// 空态图标（icons.js 注册名；会话 55：空态从纯文字升级为图标 + 文案）
const EMPTY_ICON = {
  all: "download",
  running: "bolt",
  failed: "alert-triangle",
  complete: "check-circle",
  deleted: "trash",
};

const EMPTY = {
  all: "还没有下载任务。<br />按下载热键，或点右上角「新建」开始。",
  running: "没有正在进行的任务。",
  failed: "没有失败的任务。",
  complete: "还没有完成的任务。",
  deleted: "没有已删除文件的任务。",
};

// ── 格式化 ──────────────────────────────────────────────────────────────

function fmtBytes(n) {
  n = Number(n) || 0;
  if (n < 1024) return n + " B";
  const u = ["KB", "MB", "GB", "TB"];
  let i = -1;
  do { n /= 1024; i++; } while (n >= 1024 && i < u.length - 1);
  return n.toFixed(n < 10 ? 1 : 0) + " " + u[i];
}

function fmtSpeed(n) { return fmtBytes(n) + "/s"; }

function fmtEta(s) {
  if (s == null) return "—";
  s = Number(s) || 0;
  if (s < 60) return "约 " + s + " 秒";
  if (s < 3600) return "约 " + Math.ceil(s / 60) + " 分钟";
  return "约 " + (s / 3600).toFixed(1) + " 小时";
}

const STATUS = {
  active: "下载中", waiting: "排队中", paused: "已暂停",
  complete: "已完成", error: "出错", removed: "已移除",
};
const statusText = (s) => STATUS[s] || s || "未知";

function pct(t) {
  if (!t.total_length) return 0;                    // BT 取 metadata 阶段总长为 0
  return Math.min(100, Math.round((t.completed_length / t.total_length) * 100));
}

function errText(e) {
  if (e == null) return "未知错误";
  if (typeof e === "string") return e;
  if (e.message) return String(e.message);
  return String(e);
}

function showError(msg) {
  lastError = msg || "";
  const box = el("error");
  box.textContent = lastError;
  box.classList.toggle("hidden", !lastError || filter === "github");
}

// ── 分类 ────────────────────────────────────────────────────────────────

/// 一条任务属于哪个分类。「已删除」是从「已完成」里拆出来的：
/// 状态仍是 complete，只是文件已经不在磁盘上。
function catOf(t) {
  if (t.status === "complete") return t.file_missing ? "deleted" : "complete";
  if (t.status === "error" || t.status === "removed") return "failed";
  return "running";
}

function countByCat(tasks) {
  const n = { all: tasks.length, running: 0, failed: 0, complete: 0, deleted: 0 };
  for (const t of tasks) n[catOf(t)] += 1;
  return n;
}

function setFilter(cat) {
  filter = cat;
  const isGithub = cat === "github";
  for (const b of document.querySelectorAll("button.cat")) {
    b.classList.toggle("active", b.dataset.cat === cat);
  }
  el("title").textContent = TITLES[cat] || "全部";
  // GitHub 是独立视图，不是任务过滤器：切换时把任务列表的头部按钮、
  // 空态、底栏一起藏起来，避免两套界面叠在一起。
  el("list").classList.toggle("hidden", isGithub);
  el("githubView").classList.toggle("hidden", !isGithub);
  el("newTask").classList.toggle("hidden", isGithub);
  el("clear").classList.toggle("hidden", isGithub);
  el("taskFoot").classList.toggle("hidden", isGithub);
  if (isGithub) {
    el("empty").classList.add("hidden");
    el("error").classList.add("hidden");
    window.GithubView?.show();
  } else {
    el("error").classList.toggle("hidden", !lastError);
    render(lastSnap);
  }
  const tb = document.querySelector(".tb-title");
  if (tb) tb.textContent = isGithub ? "GitHub 加速" : "任务列表";
}

// GitHub 视图下载成功后切回任务列表的「全部」分类。
window.DownloadView = { showAll: () => setFilter("all") };

// ── 行节点：只在任务首次出现时创建一次 ──────────────────────────────────

function createRow(t) {
  const row = document.createElement("div");
  row.className = "row";
  row.dataset.gid = t.gid;

  const top = document.createElement("div");
  top.className = "row-top";
  const name = document.createElement("span");
  name.className = "name";
  const p = document.createElement("span");
  p.className = "pct";
  top.append(name, p);

  const bar = document.createElement("div");
  bar.className = "bar";
  const fill = document.createElement("i");
  bar.appendChild(fill);

  const meta = document.createElement("div");
  meta.className = "meta";

  const acts = document.createElement("div");
  acts.className = "acts";
  for (const [act, label] of [["toggle", "暂停"], ["remove", "删除"], ["open", "打开目录"]]) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "btn";
    b.dataset.act = act;
    b.textContent = label;
    // 「删除任务时同时删文件」打开时，把后果写在 tooltip 上——按钮上放不下，
    // 但用户按下之前必须知道文件也会没。
    if (act === "remove" && uiFlags.delete_file) {
      b.title = uiFlags.delete_mode === "permanent"
        ? "会连同文件一起彻底删除（不可恢复）"
        : "会连同文件一起移到回收站";
    }
    acts.appendChild(b);
  }
  row.append(top, bar, meta, acts);
  return row;
}

/// 每次快照只改文本 / 宽度 / 按钮状态，不重建节点。
function updateRow(row, t) {
  const p = pct(t);
  const deleted = t.status === "complete" && t.file_missing;

  // 文件已被删除：整行变灰 + 文件名加删除线（CSS .row.deleted）
  row.classList.toggle("deleted", !!deleted);

  const nameEl = row.querySelector(".name");
  nameEl.textContent = t.name || "未知文件";
  nameEl.title = t.file_path || t.name || "";

  row.querySelector(".pct").textContent = t.total_length ? p + "%" : "—";
  row.querySelector(".bar > i").style.width = p + "%";

  const meta = row.querySelector(".meta");
  const parts = [];
  if (t.historical) parts.push("历史记录");
  parts.push(deleted ? "文件已删除" : statusText(t.status));
  if (t.status === "active") {
    parts.push(fmtSpeed(t.download_speed), "剩余 " + fmtEta(t.eta_seconds));
  }
  parts.push(fmtBytes(t.completed_length) + " / " + (t.total_length ? fmtBytes(t.total_length) : "—"));
  if (t.error_message) parts.push(t.error_message);
  meta.textContent = parts.join("  ·  ");
  meta.classList.toggle("bad", t.status === "error");

  // 按钮会随状态在 toggle / resume 之间改名，按 data-act="toggle" 找会在
  // 暂停过一次之后找不到节点。取第一个按钮，与它当前叫什么无关。
  const toggle = row.querySelector(".acts > button:first-child");
  const paused = t.status === "paused";
  const finished = t.status === "complete" || t.status === "error" || t.status === "removed";
  toggle.textContent = finished ? "已结束" : (paused ? "继续" : "暂停");
  toggle.dataset.act = paused ? "resume" : "toggle";
  toggle.disabled = finished;
}

// ── 快照渲染 ────────────────────────────────────────────────────────────

function render(snap) {
  if (!snap || !Array.isArray(snap.tasks)) return;
  lastSnap = snap;

  const seen = new Set();
  let visible = 0;
  for (const t of snap.tasks) {
    seen.add(t.gid);
    let row = rows.get(t.gid);
    if (!row) {
      row = createRow(t);
      rows.set(t.gid, row);
      el("list").appendChild(row);
    }
    updateRow(row, t);
    const show = filter === "all" || catOf(t) === filter;
    row.classList.toggle("hidden-row", !show);
    if (show) visible += 1;
  }
  for (const [gid, row] of rows) {
    if (!seen.has(gid)) { row.remove(); rows.delete(gid); }
  }

  const n = countByCat(snap.tasks);
  for (const k of ["all", "running", "failed", "complete", "deleted"]) {
    el("c-" + k).textContent = n[k];
  }
  // 主菜单「下载」上的徽标：进行中数量，0 时隐藏（会话 55，复用 countByCat）
  const badge = el("badgeDownload");
  badge.textContent = n.running;
  badge.classList.toggle("hidden", !n.running);

  el("empty").classList.toggle("hidden", visible > 0);
  if (visible === 0) {
    el("empty").innerHTML =
      '<span class="empty-ico">' +
      (Icons.raw(EMPTY_ICON[filter] || "download", 30) || "") +
      "</span>" +
      (EMPTY[filter] || EMPTY.all);
  }

  const g = snap.global || {};
  el("global").textContent = "↓ " + fmtSpeed(g.download_speed) + "　↑ " + fmtSpeed(g.upload_speed);
  el("counts").textContent = snap.tasks.length + " 个任务 · " + (g.num_active || 0) + " 个活动";

  // GitHub 视图打开时，任务列表整体隐藏；不要让这里的空态/可见数把
  // `#empty` 又翻出来盖在解析页上。
  if (filter === "github") {
    el("empty").classList.add("hidden");
    return;
  }
}

// ── 命令：失败一律显示中文原因，不静默吞掉 ─────────────────────────────

async function call(cmd, args) {
  try {
    await invoke(cmd, args || {});
    showError("");
    await refresh();                       // 不等下个轮询周期，立刻刷新
  } catch (e) {
    showError(errText(e));
  }
}

async function refresh() {
  try { render(await invoke("download_snapshot")); }
  catch (e) { showError(errText(e)); }
}

el("list").addEventListener("click", (ev) => {
  const btn = ev.target.closest("button[data-act]");
  if (!btn || btn.disabled) return;
  const row = btn.closest("[data-gid]");
  if (!row) return;
  const gid = row.dataset.gid;
  switch (btn.dataset.act) {
    case "toggle": call("download_pause", { gid }); break;
    case "resume": call("download_unpause", { gid }); break;
    case "remove": onRemove(btn, row, gid); break;
    case "open":   call("download_open_dir", { gid }); break;
  }
});

// ── 删除的二次确认 ──────────────────────────────────────────────────────
//
// 不用 `confirm()`：那是 WebView2 的系统对话框（白底、跟深色主题完全冲突，
// 违反 13-UI 的「原生部件必须自绘」）。改成「点一下按钮变成『确认删除』，
// 3 秒内再点一次才真删」——零弹窗、可撤销（不动手就自己复原）。

const armTimers = new Map();

function disarm(btn, row) {
  const t = armTimers.get(row);
  if (t) { clearTimeout(t); armTimers.delete(row); }
  btn.dataset.armed = "";
  btn.textContent = "删除";
  btn.classList.remove("armed");
}

function onRemove(btn, row, gid) {
  if (!uiFlags.confirm) {
    call("download_remove", { gid });
    return;
  }
  if (btn.dataset.armed === "1") {
    disarm(btn, row);
    call("download_remove", { gid });
    return;
  }
  btn.dataset.armed = "1";
  btn.textContent = "确认删除";
  btn.classList.add("armed");
  armTimers.set(row, setTimeout(() => disarm(btn, row), 3000));
}

el("clear").addEventListener("click", () => call("download_clear_finished"));

// 打开新建任务窗口时**不关这个窗口**（两窗并存），新建完成由后端负责关新建窗。
el("newTask").addEventListener("click", () => {
  invoke("open_new_task").catch((e) => showError(errText(e)));
});

for (const b of document.querySelectorAll("button.cat")) {
  b.addEventListener("click", () => setFilter(b.dataset.cat));
}

(async function init() {
  await Theme.apply(); // 外观：主题三态 + 主色调
  el("title").textContent = TITLES[filter] || "全部";
  // 先取界面开关再渲染：删除按钮的提示要在第一批行建出来之前就定好。
  try {
    uiFlags = await invoke("download_ui_flags");
  } catch (e) {
    /* 读不到就用默认值（要确认、不删文件） */
  }
  await refresh();
  // 订阅/渲染都不许静默失败：事件收不到或载荷形状变了，用户必须能在窗口里
  // 看到原因，而不是对着一个永远不动的进度条猜。
  try {
    await listen("tasks:snapshot", (ev) => {
      try {
        render(ev.payload);
        if (lastError) showError("");    // 后端恢复后清掉旧的错误横幅
      } catch (e) {
        showError("任务快照渲染失败：" + errText(e));
      }
    });
  } catch (e) {
    showError("订阅任务事件失败：" + errText(e));
  }
  // 刷新只靠 pump 的 tasks:snapshot 推送 + 初始化时的 download_snapshot 一次拉取，
  // 不再加定时器：实测（PrintWindow 连续两帧 7%→18%）事件通道工作正常。
  // 「文件已删除」的判定在后端 snapshot 里做（对 complete 任务 stat 一次），
  // 所以不需要前端额外轮询文件系统。
  window.TitleBar?.reveal?.();
})();

// ── 局域网互传（S4b）────────────────────────────────────────────────────
//
// 数据面：transfer_peers / transfer_pending 拉全量，transfer:event 推增量
// （风险 71：事件只是通知，丢了不影响正确性）。发送走 transfer_pick_files
// （系统对话框）或拖放（transfer:dropped → transfer_take_drop）。

const tfView = {
  view: "download",
  sub: "send",
  peers: [],
  rows: new Map(),   // `${sessionId}|${fileName}` → row 元素
  dropPaths: [],
  receiveDir: "",
};

const downloadView = el("downloadView");
const tbTitle = document.querySelector(".tb-title");

const DEVICE_ICON = {
  mobile: "device-phone",
  desktop: "device-desktop",
  web: "device-web",
  server: "device-server",
  headless: "device-headless",
};

function deviceIcon(type) {
  return DEVICE_ICON[type] || "device-desktop";
}

function setView(v) {
  tfView.view = v;
  const isT = v === "transfer";
  for (const b of document.querySelectorAll(".nav-item[data-view]")) {
    b.classList.toggle("active", b.dataset.view === v);
  }
  downloadView.classList.toggle("hidden", isT);
  el("catsWrap").classList.toggle("hidden", isT);
  el("subTransfer").classList.toggle("hidden", !isT);
  el("transferView").classList.toggle("hidden", !isT);
  if (tbTitle) tbTitle.textContent = isT ? "局域网互传" : "任务列表";
  if (isT) {
    setSub(tfView.sub || "send");
    refreshPeers();
    refreshPending();
    refreshLocalInfo();
  } else {
    // 从互传切回来时恢复上次的下载分类（可能是 GitHub 加速视图）。
    setFilter(filter);
  }
}

function setSub(sub) {
  tfView.sub = sub;
  for (const b of document.querySelectorAll(".sub-item[data-sub]")) {
    b.classList.toggle("active", b.dataset.sub === sub);
  }
  el("paneSend").classList.toggle("hidden", sub !== "send");
  el("paneReceive").classList.toggle("hidden", sub !== "receive");
  el("paneHistory").classList.toggle("hidden", sub !== "history");
  if (sub === "receive") {
    refreshPending();
    refreshLocalInfo();
  } else if (sub === "history") {
    hsLoad(1);
  }
}

async function refreshLocalInfo() {
  try {
    const info = await invoke("transfer_local_info");
    tfView.receiveDir = info.receive_dir || "";
    el("localAlias").textContent = info.alias || "—";
    const ip = (info.ips && info.ips[0]) || "";
    el("localAddr").textContent = ip
      ? "主机地址 " + ip
      : (info.enabled ? "未获取到局域网地址" : "互传已停用");
    el("localDir").textContent = info.receive_dir || "—";
    el("localDir").title = info.receive_dir || "";
  } catch (e) {
    el("localAddr").textContent = "读取本机信息失败：" + errText(e);
  }
  refreshAutoSave();
}

// 「自动保存」= transfer_auto_accept：开了就不再弹确认框，直接存盘。
async function refreshAutoSave() {
  try {
    const res = await invoke("get_tool_config", { toolId: "transfer" });
    const v = (res && res.values) || {};
    setAutoSave(v.transfer_auto_accept === true);
  } catch (e) {
    /* 读不到就当关着 */
  }
}

function setAutoSave(on) {
  const b = el("autoSave");
  if (b) b.setAttribute("aria-checked", on ? "true" : "false");
}

el("autoSave").addEventListener("click", async () => {
  const on = el("autoSave").getAttribute("aria-checked") !== "true";
  setAutoSave(on);
  try {
    const res = await invoke("save_tool_config", {
      toolId: "transfer",
      values: { transfer_auto_accept: on },
    });
    if (!res || !res.ok) {
      setAutoSave(!on);
      tfNotice((res && res.message) || "保存失败", true);
    } else {
      tfNotice(on ? "已开启自动保存：收到的文件直接存盘" : "已关闭自动保存：接收前会先弹确认");
    }
  } catch (e) {
    setAutoSave(!on);
    tfNotice("保存失败：" + errText(e), true);
  }
});

// 操作结果统一走标题栏下方的 Toast（toast.js）；持久错误仍走页面内的 #error。
function tfNotice(msg, danger) {
  if (!msg) return;
  window.Toast.show(msg, { kind: danger ? "error" : "info" });
}

function renderPeers() {
  const box = el("peerList");
  box.innerHTML = "";
  el("peerEmpty").classList.toggle("hidden", tfView.peers.length > 0);
  for (const p of tfView.peers) {
    const d = document.createElement("div");
    d.className = "device-item";

    const av = document.createElement("span");
    av.className = "device-avatar";
    av.innerHTML = Icons.svg(deviceIcon(p.device_type), 20);

    const meta = document.createElement("div");
    meta.className = "device-meta";
    const name = document.createElement("div");
    name.className = "device-name";
    name.textContent = p.alias || "未知设备";
    const sub = document.createElement("div");
    sub.className = "device-sub";
    const bits = [p.ip + ":" + p.port];
    if (p.device_model) bits.push(p.device_model);
    if (p.protocol) bits.push(p.protocol);
    sub.textContent = bits.join(" · ");
    meta.append(name, sub);

    // 整行就是发送入口：选好文件后点设备直接发（不再单放「发送文件」按钮）
    d.tabIndex = 0;
    d.setAttribute("role", "button");
    d.title = "发送给 " + (p.alias || p.ip);
    const go = document.createElement("span");
    go.className = "device-go";
    go.innerHTML = Icons.svg("upload", 16);
    d.append(av, meta, go);
    d.addEventListener("click", () => sendToPeer(p));
    d.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter" || ev.key === " ") {
        ev.preventDefault();
        sendToPeer(p);
      }
    });
    box.appendChild(d);
  }
}

async function refreshPeers() {
  try {
    tfView.peers = (await invoke("transfer_peers")) || [];
    renderPeers();
  } catch (e) {
    tfNotice("读取设备列表失败：" + errText(e), true);
  }
}

function fmtTfBytes(n) { return fmtBytes(n); }

// ── 收到传输请求：互传窗口内的遮罩子对话框（会话 80） ──────────────────
//
// 用户要求确认框是「互传窗口的子窗口 + 遮罩层」，不再是独立窗口。
// 数据仍然走 transfer_pending 拉全量；每 1.2 秒拉一次，对方点「取消」后
// 后端会把请求从待确认列表里剔掉，这里发现列表空了就自动关框。

const cf = { req: null, timer: null, busy: false };

function shortIp(ip) {
  if (!ip) return "#—";
  const parts = String(ip).split(".");
  return "#" + (parts.length > 1 ? parts[parts.length - 1] : ip);
}

function cfError(msg) {
  el("cfError").textContent = msg || "";
  el("cfError").classList.toggle("hidden", !msg);
}

function cfSetBusy(on) {
  el("cfAccept").disabled = on;
  el("cfReject").disabled = on;
  el("cfClose").disabled = on;
}

function cfRenderFiles(files) {
  const ul = el("cfFileList");
  ul.innerHTML = "";
  const shown = files.slice(0, 4);
  for (const f of shown) {
    const li = document.createElement("li");
    const ico = document.createElement("span");
    ico.className = "ico";
    ico.innerHTML = Icons.svg("file", 14);
    const n = document.createElement("span");
    n.className = "n";
    n.textContent = f.name;
    n.title = f.name;
    li.append(ico, n);
    ul.appendChild(li);
  }
  if (files.length > shown.length) {
    const li = document.createElement("li");
    li.className = "more";
    li.textContent = "……";
    ul.appendChild(li);
  }
}

function renderConfirm(req) {
  cf.req = req;
  cfError("");
  cfSetBusy(false);

  el("cfIcon").innerHTML = Icons.svg(deviceIcon(req.sender_type), 20);
  el("cfSender").textContent = req.sender_alias || "未知设备";
  el("cfIp").textContent = shortIp(req.sender_ip);
  el("cfIp").title = req.sender_ip || "";
  el("cfModel").textContent = req.sender_model || "未知型号";

  const count = req.files.length;
  el("cfCount").textContent = count === 1 ? "一个文件" : count + " 个文件";
  el("cfPreviewText").textContent =
    count === 1 ? req.files[0].name : "查看接收文件列表 (" + count + ")";
  el("cfPreviewText").title = count === 1 ? req.files[0].name : "";
  cfRenderFiles(req.files);
  el("cfTotal").textContent = "共 " + fmtTfBytes(req.total_bytes);
  el("cfPath").textContent = tfView.receiveDir || "—";
  el("cfPath").title = tfView.receiveDir || "";
}

function openConfirm(req) {
  if (!req) return;
  if (cf.req && cf.req.session_id === req.session_id) return;
  renderConfirm(req);
  el("cfLayer").classList.remove("hidden");
  if (!cf.timer) cf.timer = setInterval(pollIncoming, 1200);
  // 接收目录可能还没拉过，顺手刷新一次
  if (!tfView.receiveDir) refreshLocalInfo();
}

function closeConfirm(msg) {
  el("cfLayer").classList.add("hidden");
  if (cf.timer) {
    clearInterval(cf.timer);
    cf.timer = null;
  }
  cf.req = null;
  cf.busy = false;
  cfError("");
  if (msg) tfNotice(msg);
}

/// 轮询待确认列表：对方撤销后列表里会没有当前请求，此时自动关框。
async function pollIncoming() {
  const cur = cf.req;
  if (!cur || cf.busy) return;
  try {
    const list = (await invoke("transfer_pending")) || [];
    // 请求可能在 await 期间被应答 / 换掉，别再动已经不是当前这条的界面
    if (cf.req !== cur || cf.busy) return;
    if (!list.length) {
      closeConfirm("对方已取消传输");
      return;
    }
    if (!list.some((r) => r.session_id === cur.session_id)) {
      // 当前这条已失效，还有别的请求就换过去，否则关框
      renderConfirm(list[0]);
    }
  } catch (e) {
    /* 网络/服务临时不可用时保持现状，下一轮再试 */
  }
}

async function respondIncoming(accept) {
  if (!cf.req || cf.busy) return;
  cf.busy = true;
  cfError("");
  cfSetBusy(true);
  try {
    await invoke("transfer_respond", {
      sessionId: cf.req.session_id,
      accept,
    });
    const list = (await invoke("transfer_pending")) || [];
    cf.busy = false;
    if (list.length) {
      renderConfirm(list[0]);
      cfSetBusy(false);
    } else {
      closeConfirm();
    }
  } catch (e) {
    cf.busy = false;
    cfSetBusy(false);
    const msg = errText(e);
    // 对方刚撤销 / 已被处理：关框比留一个点不动的按钮更合理
    if (msg.includes("取消") || msg.includes("过期")) {
      closeConfirm("传输请求已失效");
    } else {
      cfError(msg);
    }
  }
}

async function refreshPending() {
  try {
    const list = (await invoke("transfer_pending")) || [];
    tfView.pending = list;
    if (list.length) {
      openConfirm(list[0]);
    } else if (cf.req && !cf.busy) {
      closeConfirm("对方已取消传输");
    }
  } catch (e) {
    /* 确认框是主路径，这里失败只记提示 */
    tfNotice("读取待确认请求失败：" + errText(e), true);
  }
}

el("cfAccept").addEventListener("click", () => respondIncoming(true));
el("cfReject").addEventListener("click", () => respondIncoming(false));
el("cfClose").addEventListener("click", () => respondIncoming(false));
el("cfOpenDir").addEventListener("click", () => {
  if (!tfView.receiveDir) return;
  invoke("open_path", { path: tfView.receiveDir }).catch((e) => cfError(errText(e)));
});
Icons.fill(el("cfClose"), "close", 14);
Icons.fill(el("cfOpenDir"), "folder", 16);
el("cfPreviewIcon").className = "ico";
el("cfPreviewIcon").innerHTML = Icons.svg("file", 14);

function upsertRow(view) {
  // 发送页已不列「传输进度」（进度全在覆盖整窗的传输页里），容器没了就别写
  const box = el("tfList");
  if (!box) return;
  const key = view.session_id + "|" + view.file_name;
  let row = tfView.rows.get(key);
  if (!row) {
    row = document.createElement("div");
    row.className = "tf-row";
    row.innerHTML =
      '<div class="line"><span class="tf-dir"></span><span class="fname"></span><span class="fstat"></span></div>' +
      '<div class="bar"><i></i></div>';
    box.appendChild(row);
    tfView.rows.set(key, row);
  }
  row.classList.toggle("done", view.status === "done");
  row.classList.toggle("failed", view.status === "failed" || view.status === "cancelled");
  // 方向用图标，不用 ↑ / ↓ 字符（13-UI §13.5：可用图形的地方不拿符号凑）
  const dir = row.querySelector(".tf-dir");
  if (!dir.dataset.set) {
    const sending = view.direction === "send";
    dir.innerHTML = Icons.svg(sending ? "upload" : "download", 14);
    dir.title = sending ? "发送" : "接收";
    dir.dataset.set = "1";
  }
  row.querySelector(".fname").textContent = view.file_name || "(会话)";
  const pct = view.total > 0 ? Math.min(100, Math.round((view.bytes / view.total) * 100)) : (view.status === "done" ? 100 : 0);
  let stat = pct + "%";
  if (view.speed_bps) stat += " · " + fmtBytes(view.speed_bps) + "/s";
  if (view.status === "preparing") stat = view.message || "准备中…";
  else if (view.status === "done" && view.message) stat = "完成 · " + view.message;
  else if (view.status === "failed") stat = "失败 · " + (view.message || "");
  else if (view.status === "cancelled") stat = "已取消";
  row.querySelector(".fstat").textContent = stat;
  row.querySelector(".bar i").style.width = pct + "%";
  // 发送中的行给一个取消按钮
  if (view.direction === "send" && view.status === "active") {
    if (!row.querySelector("[data-cancel]")) {
      const b = document.createElement("button");
      b.className = "btn";
      b.dataset.cancel = "1";
      b.textContent = "取消";
      b.addEventListener("click", () =>
        invoke("transfer_cancel_send", { sessionId: view.session_id }).catch((e) =>
          tfNotice(errText(e), true)
        )
      );
      row.querySelector(".line").appendChild(b);
    }
  } else {
    const b = row.querySelector("[data-cancel]");
    if (b) b.remove();
  }
}

// ── 选文件卡片（S4 · 会话 46） ─────────────────────────────────────────
// 拖入或点「添加」都往 tfView.dropPaths 里堆，卡片上直接摊开「文件 N 个 / 大小 X」；
// 选好之后点下面任意一台设备就发出去——不再有「将发送 N 个文件到…」的确认条，
// 设备行也不再单放「发送文件」按钮。
async function addFiles() {
  try {
    const paths = await invoke("transfer_pick_files");
    if (!paths || !paths.length) return;
    for (const p of paths) {
      if (!tfView.dropPaths.includes(p)) tfView.dropPaths.push(p);
    }
    await renderPick();
  } catch (e) {
    tfNotice("选择文件失败：" + errText(e), true);
  }
}

function addDropped(paths) {
  if (!paths || !paths.length) return;
  for (const p of paths) {
    if (!tfView.dropPaths.includes(p)) tfView.dropPaths.push(p);
  }
  renderPick();
}

async function renderPick() {
  const paths = tfView.dropPaths || [];
  el("pickCount").textContent = "文件：" + paths.length;
  el("pickHint").classList.toggle("hidden", paths.length > 0);
  el("pickClear").classList.toggle("hidden", paths.length === 0);

  let sizes = [];
  if (paths.length) {
    try {
      sizes = await invoke("transfer_path_sizes", { paths });
    } catch (e) {
      sizes = [];
    }
  }
  el("pickSize").textContent =
    "大小：" + fmtBytes(sizes.reduce((a, b) => a + (b || 0), 0));

  const grid = el("pickGrid");
  grid.innerHTML = "";
  paths.forEach((p, i) => {
    const name = p.replace(/^.*[\\/]/, "");
    const item = document.createElement("div");
    item.className = "pick-item";
    item.innerHTML =
      '<div class="pick-tile">' + Icons.svg("file", 40) + "</div>" +
      '<div class="pick-name"></div>' +
      '<button class="pick-del" type="button" title="移除这个文件" aria-label="移除"></button>';
    const nm = item.querySelector(".pick-name");
    nm.textContent = name;
    nm.title = p;
    // 单击文件名展开完整名字，再点收回去
    nm.addEventListener("click", (ev) => {
      ev.stopPropagation();
      item.classList.toggle("open");
    });
    const del = item.querySelector(".pick-del");
    // 设计稿的粉红圆形「×」，无边框；28px 比原来的 22px 更醒目
    del.innerHTML = Icons.raw("deleteRound", 28);
    del.addEventListener("click", (ev) => {
      ev.stopPropagation();
      tfView.dropPaths.splice(i, 1);
      renderPick();
    });
    grid.appendChild(item);
  });
}

function clearPick() {
  tfView.dropPaths = [];
  renderPick();
}

// 点设备 = 直接发；没选文件就先提示，不去猜用户想发什么
async function sendToPeer(peer) {
  const paths = (tfView.dropPaths || []).slice();
  if (!paths.length) {
    tfNotice("先从上面的卡片里添加要发送的文件");
    return;
  }
  tfView.dropPaths = [];
  renderPick();
  await doSend(peer.fingerprint || (peer.ip + ":" + peer.port), paths, peer);
}

async function doSend(target, paths, peer) {
  if (!paths || !paths.length) return;
  const pin = peer && peer.pin ? peer.pin : undefined;
  try {
    tfNotice("正在发送 " + paths.length + " 个文件…");
    await invoke("transfer_send", { target, paths, pin });
  } catch (e) {
    // 对方开了 PIN 时这里会失败——提示用户在对方处关闭 PIN 或线下告知
    tfNotice("发送失败：" + errText(e), true);
  }
}

el("tfRefresh").addEventListener("click", async () => {
  try {
    await invoke("transfer_refresh");
    tfNotice("正在刷新设备…");
    await refreshPeers();
  } catch (e) {
    tfNotice("刷新失败：" + errText(e), true);
  }
});
el("tfRefreshRecv").addEventListener("click", () => { refreshPending(); refreshLocalInfo(); });
el("pickAdd").addEventListener("click", () => addFiles());
el("pickClear").addEventListener("click", () => clearPick());
// 拖着文件进窗口、还没松手时：整张选择卡片换成「松开以添加文件」页。
// 用 document 级事件 + 计数，避免鼠标在子元素间移动时反复闪。
el("pickDropIcon").innerHTML = Icons.raw("bigDownload", 96);

let dragDepth = 0;
function draggingFiles(e) {
  const types = e.dataTransfer && e.dataTransfer.types;
  return !!types && Array.from(types).includes("Files");
}
function showPickDrop() {
  el("pickDrop").classList.remove("hidden");
}
function hidePickDrop() {
  dragDepth = 0;
  el("pickDrop").classList.add("hidden");
}
document.addEventListener("dragenter", (e) => {
  if (!draggingFiles(e)) return;
  e.preventDefault();
  dragDepth += 1;
  showPickDrop();
});
document.addEventListener("dragover", (e) => {
  if (!draggingFiles(e)) return;
  e.preventDefault();
  showPickDrop();
});
document.addEventListener("dragleave", (e) => {
  if (!draggingFiles(e)) return;
  dragDepth = Math.max(0, dragDepth - 1);
  if (dragDepth === 0) hidePickDrop();
});
document.addEventListener("drop", (e) => {
  e.preventDefault();
  hidePickDrop();
});
el("tfOpenDir").addEventListener("click", () => {
  if (!tfView.receiveDir) { tfNotice("还没有读取到接收目录", true); return; }
  invoke("open_path", { path: tfView.receiveDir }).catch((e) => tfNotice(errText(e), true));
});

for (const b of document.querySelectorAll(".nav-item[data-view]")) {
  b.addEventListener("click", () => setView(b.dataset.view));
}
for (const b of document.querySelectorAll(".sub-item[data-sub]")) {
  b.addEventListener("click", () => setSub(b.dataset.sub));
}

// ── 传输页（S4 · 会话 45） ──────────────────────────────────────────────
//
// 数据源有两条：①开场 `transfer_active()` 拉一次快照——窗口中途打开、或者
// 传输在窗口关着的时候就开始了，只能靠它还原进度（事件是「当场」推的，错过就没了）；
// ②之后由 `transfer:event` 的 progress 增量更新。
const tpView = {
  sessions: [],      // 最近 4 个会话的快照（后端也是这个上限）
  open: false,
  autoComplete: false,
  pendingCancel: null, // 等待二次确认的 session_id
  dismissed: null,     // 用户点过「已完成/关闭」的会话，不再自动弹回
};

function tpCurrent() {
  // 后端按「新会话 push 到尾部」维护，取最后一个就是本次传输
  return tpView.sessions.length ? tpView.sessions[tpView.sessions.length - 1] : null;
}

function tpStatusOf(s) {
  if (!s) return "idle";
  if (s.status === "done") return "done";
  if (s.status === "cancelled") return "cancelled";
  if (s.status === "failed") return "failed";
  return s.status === "preparing" ? "preparing" : "active";
}

function tpPct(s) {
  if (!s) return 0;
  if (s.status === "done") return 100;
  if (!s.total) return 0;
  return Math.min(100, Math.round((s.bytes / s.total) * 100));
}

function tpFilePct(f) {
  if (!f.total) return f.status === "done" ? 100 : 0;
  return Math.min(100, Math.round((f.bytes / f.total) * 100));
}

function tpRender() {
  const s = tpCurrent();
  const page = el("tp");
  if (!s) { page.classList.add("hidden"); tpView.open = false; return; }

  const st = tpStatusOf(s);
  const sending = s.direction === "send";
  const peer = s.peer || "对方";

  // 标题 / 副标题
  let title = sending ? "正在发送文件" : "正在接收文件";
  if (st === "preparing") title = "正在准备传输";
  else if (st === "done") title = "传输完成";
  else if (st === "cancelled") title = "已取消";
  else if (st === "failed") title = "传输失败";
  el("tpTitle").textContent = title;

  const sub = sending
    ? "发送给 " + peer
    : "来自 " + peer + (tfView.receiveDir ? "　·　保存到 " + tfView.receiveDir : "");
  el("tpSub").textContent = sub;

  // 文件行
  const box = el("tpFiles");
  box.innerHTML = "";
  const files = s.files && s.files.length
    ? s.files
    : [{ name: s.message || "（会话）", bytes: s.bytes, total: s.total, status: st }];
  const cancelled = st === "cancelled";
  for (const f of files) {
    const row = document.createElement("div");
    const rowFailed = f.status === "failed" && !cancelled;
    row.className = "tp-file" + (f.status === "done" ? " done" : rowFailed ? " failed" : "");
    row.innerHTML =
      '<span class="tp-file-ico">' + Icons.svg("file", 20) + "</span>" +
      '<div class="tp-file-meta">' +
        '<div class="tp-file-name"></div>' +
        '<div class="tp-file-sub"></div>' +
        '<div class="tp-file-bar"><i></i></div>' +
      "</div>" +
      '<span class="tp-file-pct num"></span>';
    row.querySelector(".tp-file-name").textContent = f.name || "（会话）";
    const pct = tpFilePct(f);
    const bits = [];
    if (f.status === "done") bits.push(fmtBytes(f.total || f.bytes));
    else bits.push(fmtBytes(f.bytes) + " / " + fmtBytes(f.total));
    row.querySelector(".tp-file-sub").textContent = bits.join("　");
    row.querySelector(".tp-file-bar i").style.width = pct + "%";
    // 整个会话被取消时，行上直接说清「谁取消的」，比停在某个百分比有用
    row.querySelector(".tp-file-pct").textContent = cancelled && f.status !== "done"
      ? (sending ? "已取消" : "对方已取消")
      : rowFailed
        ? "失败"
        : pct + "%";
    box.appendChild(row);
  }

  // 底部总进度
  const pct = tpPct(s);
  el("tpBar").style.width = pct + "%";
  let stat = pct + "%";
  if (s.total) stat = fmtBytes(s.bytes) + " / " + fmtBytes(s.total) + "　·　" + pct + "%";
  if (st === "active" && s.speed_bps) stat += "　·　" + fmtBytes(s.speed_bps) + "/s";
  if (st === "preparing") stat = "准备中…";
  if (st === "done") stat = "100%　·　已完成";
  if (st === "cancelled") stat = "已取消";
  if (st === "failed") stat = "失败" + (s.message ? "　·　" + s.message : "");
  el("tpStat").textContent = stat;

  // 主按钮：传输中=取消，结束=关闭
  const act = el("tpAction");
  const finished = st === "done" || st === "cancelled" || st === "failed";
  act.textContent = finished ? (st === "done" ? "已完成" : "关闭") : "取消";
  act.classList.toggle("btn-primary", !finished || st === "done");

  page.classList.remove("hidden");
  tpView.open = true;
  const tb = document.querySelector(".tb-title");
  if (tb) tb.textContent = "传输";

  // 「自动完成」：传完停一会儿再收页面，让用户看到「已完成」；关掉则等用户点
  if (st === "done" && tpView.autoComplete && !s._autoClosed) {
    s._autoClosed = true;
    setTimeout(() => {
      if (tpView.dismissed === s.session_id) return;
      tpClose();
    }, 1200);
  }
}

function tpSync(list) {
  tpView.sessions = Array.isArray(list) ? list : [];
  tpRender();
}

// 新会话出现 / 进度推进：更新快照并按需打开页面
function tpOnProgress(view) {
  if (!view || !view.session_id) return;
  const sid = view.session_id;
  let s = tpView.sessions.find((x) => x.session_id === sid);
  if (!s) {
    s = {
      session_id: sid,
      direction: view.direction,
      peer: "",
      status: view.status,
      message: view.message || null,
      bytes: 0,
      total: 0,
      speed_bps: null,
      files: [],
    };
    tpView.sessions.push(s);
    if (tpView.sessions.length > 4) tpView.sessions.shift();
    // 用户主动关掉的会话不再自动弹回来
    if (tpView.dismissed !== sid) {
      tpView.dismissed = null;
      renderTpWith(s);
      return;
    }
  }
  // 已在跟踪：用事件里的数据就地更新（事件带的不全，缺的字段保留旧值）
  const sessionLevel = !view.file_name || view.file_name.endsWith(" 个文件");
  // 会话状态只认会话级事件：文件级的 failed/done 不能把「已取消」冲掉
  if (sessionLevel) {
    if (view.status) s.status = view.status;
    if (view.message) s.message = view.message;
    if (view.speed_bps) s.speed_bps = view.speed_bps;
    if (view.total > 0) { s.total = view.total; s.bytes = view.bytes; }
  } else {
    if (view.speed_bps) s.speed_bps = view.speed_bps;
    if (view.message && !s.message) s.message = view.message;
    let f = s.files.find((x) => x.name === view.file_name);
    if (!f) {
      f = { name: view.file_name, bytes: view.bytes, total: view.total, status: view.status };
      s.files.push(f);
    } else {
      f.bytes = view.bytes;
      if (view.total > 0) f.total = view.total;
      if (["done", "failed", "cancelled"].includes(view.status)) f.status = view.status;
    }
    if (s.status !== "cancelled" && s.status !== "failed") {
      s.total = s.files.reduce((a, x) => a + (x.total || 0), 0);
      s.bytes = s.files.reduce((a, x) => a + (x.bytes || 0), 0);
      // 接收侧没有会话级终态事件，靠「所有文件都完成」推断
      if (s.files.length && s.files.every((x) => x.status === "done")) s.status = "done";
    }
  }

  // 接收侧事件不带速度，用两次进度的差值在本地估一个（够看趋势，不做精确度量）
  const now = Date.now();
  if (s._lastTs && now - s._lastTs > 200) {
    const db = s.bytes - (s._lastBytes || 0);
    if (db >= 0) {
      const inst = (db / (now - s._lastTs)) * 1000;
      s._speed = s._speed ? s._speed * 0.6 + inst * 0.4 : inst;
      s.speed_bps = Math.round(s._speed);
    }
  }
  s._lastTs = now;
  s._lastBytes = s.bytes;

  if (tpView.open) tpRender();
}

function renderTpWith(s) {
  // 新会话：先拉一次全量快照（拿到 peer / 文件名），再渲染
  tpView.sessions = [s];
  tpRender();
  invoke("transfer_active").then((list) => {
    if (Array.isArray(list) && list.length) tpSync(list);
  }).catch(() => {});
}

async function tpLoad() {
  try {
    // 接收目录要在传输页也能显示（副标题「保存到 …」），别指望用户先去接收页点一下
    invoke("transfer_local_info").then((info) => {
      if (info && info.receive_dir) {
        tfView.receiveDir = info.receive_dir;
        if (tpView.open) tpRender();
      }
    }).catch(() => {});

    const list = await invoke("transfer_active");
    const active = (list || []).some(
      (s) => s.status === "active" || s.status === "preparing"
    );
    tpView.sessions = list || [];
    // 只有「正在传」才自动弹：传输早就结束了再开窗口，不该莫名弹个进度页
    if (active) tpRender();
  } catch (e) { /* 模块停用 / 服务没起：不弹即可 */ }
  tpLoadConfig();
}

async function tpLoadConfig() {
  try {
    const res = await invoke("get_tool_config", { toolId: "transfer" });
    const v = (res && res.values) || {};
    tpView.autoComplete = v.transfer_auto_complete === true;
  } catch (e) { tpView.autoComplete = false; }
}

async function tpDoCancel(s) {
  try {
    if (s.direction === "send") await invoke("transfer_cancel_send", { sessionId: s.session_id });
    else await invoke("transfer_cancel_receive", { sessionId: s.session_id });
  } catch (e) {
    tfNotice("取消失败：" + errText(e), true);
  }
}

function tpClose() {
  const s = tpCurrent();
  if (s) tpView.dismissed = s.session_id;
  el("tp").classList.add("hidden");
  el("tpConfirm").classList.add("hidden");
  tpView.open = false;
  tpView.pendingCancel = null;
  const tb = document.querySelector(".tb-title");
  if (tb) tb.textContent = "任务列表";
}

el("tpAction").addEventListener("click", () => {
  const s = tpCurrent();
  if (!s) { tpClose(); return; }
  const st = tpStatusOf(s);
  if (st === "active" || st === "preparing") {
    tpView.pendingCancel = s.session_id;
    el("tpConfirm").classList.remove("hidden");
  } else {
    tpClose();
  }
});
el("tpConfirmNo").addEventListener("click", () => {
  tpView.pendingCancel = null;
  el("tpConfirm").classList.add("hidden");
});
el("tpConfirmYes").addEventListener("click", () => {
  const s = tpCurrent();
  el("tpConfirm").classList.add("hidden");
  tpView.pendingCancel = null;
  if (s) tpDoCancel(s);
});

// ── 历史记录（S4 · 会话 45） ────────────────────────────────────────────
//
// 一条 = 一次会话（收或发）。数据在 `%APPDATA%\vexto\transfer-history.json`，
// 后端翻页（历史可能有几百条，不该整包丢给前端）。
const hsView = { page: 1, pageSize: 20, total: 0, items: [], menuId: null };

function hsPad(n) { return n < 10 ? "0" + n : String(n); }

function fmtWhen(ms) {
  const d = new Date(ms || 0);
  if (Number.isNaN(d.getTime())) return "—";
  return (
    d.getFullYear() + "-" + hsPad(d.getMonth() + 1) + "-" + hsPad(d.getDate()) +
    " " + hsPad(d.getHours()) + ":" + hsPad(d.getMinutes())
  );
}

function hsStatusText(s) {
  if (s === "done") return "已完成";
  if (s === "failed") return "失败";
  if (s === "cancelled") return "已取消";
  return s || "—";
}

function hsDirOf(p) {
  return String(p || "").replace(/[\\/][^\\/]*$/, "");
}

async function hsLoad(page) {
  if (page) hsView.page = page;
  try {
    const r = await invoke("transfer_history_page", {
      page: hsView.page,
      pageSize: hsView.pageSize,
    });
    hsView.items = (r && r.items) || [];
    hsView.total = (r && r.total) || 0;
    hsView.page = (r && r.page) || 1;
    hsRender();
  } catch (e) {
    tfNotice("读取历史失败：" + errText(e), true);
  }
}

function hsRender() {
  const box = el("hsList");
  box.innerHTML = "";
  el("hsEmpty").classList.toggle("hidden", hsView.items.length > 0);

  for (const it of hsView.items) {
    const row = document.createElement("div");
    row.className = "hs-row";
    row.innerHTML =
      '<span class="hs-dir"></span>' +
      '<div class="hs-meta"><div class="hs-name"></div><div class="hs-sub"></div></div>' +
      '<span class="hs-badge"></span>' +
      '<button class="hs-more" type="button" aria-haspopup="menu" aria-expanded="false"></button>';
    const sending = it.direction === "send";
    row.querySelector(".hs-dir").innerHTML = Icons.svg(sending ? "upload" : "download", 16);
    row.querySelector(".hs-dir").title = sending ? "发送" : "接收";

    const names = (it.files || []).map((f) => f.name).filter(Boolean);
    const first = names[0] || "（会话）";
    row.querySelector(".hs-name").textContent =
      names.length > 1 ? first + " 等 " + names.length + " 个文件" : first;

    const bits = [fmtBytes(it.total_bytes || 0)];
    bits.push(sending ? "发送给 " + (it.peer || "对方") : "来自 " + (it.peer || "对方"));
    bits.push(fmtWhen(it.at_ms));
    row.querySelector(".hs-sub").textContent = bits.join("　·　");

    const badge = row.querySelector(".hs-badge");
    badge.textContent = hsStatusText(it.status);
    if (it.status === "done") badge.classList.add("done");
    else if (it.status === "failed") badge.classList.add("failed");

    const more = row.querySelector(".hs-more");
    more.innerHTML = Icons.svg("more", 16);
    more.addEventListener("click", (ev) => {
      ev.stopPropagation();
      hsOpenMenu(it, more);
    });
    box.appendChild(row);
  }

  // 翻页
  const pages = Math.max(1, Math.ceil(hsView.total / hsView.pageSize));
  el("hsPager").classList.toggle("hidden", hsView.total === 0);
  el("hsPageInfo").textContent =
    "第 " + hsView.page + " / " + pages + " 页　·　共 " + hsView.total + " 条";
  el("hsPrev").disabled = hsView.page <= 1;
  el("hsNext").disabled = hsView.page >= pages;
}

function hsCloseMenu() {
  el("hsMenu").classList.add("hidden");
  for (const b of document.querySelectorAll(".hs-more")) b.setAttribute("aria-expanded", "false");
  hsView.menuId = null;
}

function hsOpenMenu(item, anchor) {
  const menu = el("hsMenu");
  if (hsView.menuId === item.id && !menu.classList.contains("hidden")) {
    hsCloseMenu();
    return;
  }
  hsView.menuId = item.id;
  const first = (item.files || [])[0] || {};
  const hasPath = !!(first.path && first.path.length);

  const entries = [
    { key: "open", icon: "external", text: "打开文件", disabled: !hasPath },
    { key: "reveal", icon: "folder", text: "打开所在目录", disabled: !hasPath },
    { key: "info", icon: "info", text: "文件信息", disabled: false },
    { key: "del", icon: "trash", text: "从历史删除", disabled: false, danger: true },
  ];
  menu.innerHTML = "";
  for (const e of entries) {
    const b = document.createElement("button");
    b.type = "button";
    if (e.danger) b.className = "danger";
    b.innerHTML = Icons.svg(e.icon, 16) + "<span>" + e.text + "</span>";
    if (e.disabled) {
      b.disabled = true;
      b.style.opacity = "0.45";
      b.style.cursor = "default";
    } else {
      b.addEventListener("click", () => {
        hsCloseMenu();
        if (e.key === "open") invoke("open_path", { path: first.path }).catch((err) => tfNotice(errText(err), true));
        else if (e.key === "reveal") invoke("open_path", { path: hsDirOf(first.path) }).catch((err) => tfNotice(errText(err), true));
        else if (e.key === "info") hsShowInfo(item);
        else if (e.key === "del") hsRemove(item);
      });
    }
    menu.appendChild(b);
  }

  // 贴着按钮右下角弹出；贴边时往回收，别被窗口裁掉
  menu.classList.remove("hidden");
  const r = anchor.getBoundingClientRect();
  const mw = menu.offsetWidth || 184;
  const mh = menu.offsetHeight || 160;
  let left = r.right - mw;
  let top = r.bottom + 4;
  if (left < 8) left = 8;
  if (top + mh > window.innerHeight - 8) top = Math.max(8, r.top - mh - 4);
  menu.style.left = left + "px";
  menu.style.top = top + "px";
  anchor.setAttribute("aria-expanded", "true");
}

function hsShowInfo(item) {
  const body = el("hsInfoBody");
  body.innerHTML = "";
  const rows = [];
  const files = item.files || [];
  rows.push(["方向", item.direction === "send" ? "发送" : "接收"]);
  rows.push(["对端", item.peer || "—"]);
  rows.push(["结果", hsStatusText(item.status)]);
  rows.push(["时间", fmtWhen(item.at_ms)]);
  rows.push(["总大小", fmtBytes(item.total_bytes || 0)]);
  if (files.length > 1) rows.push(["文件数", files.length + " 个"]);
  for (const f of files) {
    rows.push(["文件名", f.name || "—"]);
    rows.push(["大小", fmtBytes(f.size || 0)]);
    rows.push(["路径", f.path || "（未记录）"]);
  }
  // 备注多数时候就是落盘路径，和上面「路径」重复，就不显示了
  const paths = files.map((f) => f.path).filter(Boolean);
  if (item.message && !paths.includes(item.message)) rows.push(["备注", item.message]);

  for (const [k, v] of rows) {
    const d = document.createElement("div");
    d.className = "hs-kv-row";
    const kk = document.createElement("div");
    kk.className = "k";
    kk.textContent = k;
    const vv = document.createElement("div");
    vv.className = "v";
    vv.textContent = v;
    d.append(kk, vv);
    body.appendChild(d);
  }
  el("hsInfo").classList.remove("hidden");
}

async function hsRemove(item) {
  try {
    await invoke("transfer_history_remove", { id: item.id });
    tfNotice("已从历史删除");
    await hsLoad();
  } catch (e) {
    tfNotice("删除失败：" + errText(e), true);
  }
}

el("hsInfoClose").addEventListener("click", () => el("hsInfo").classList.add("hidden"));
document.addEventListener("click", () => hsCloseMenu());
el("hsPageSize").addEventListener("change", () => {
  hsView.pageSize = parseInt(el("hsPageSize").value, 10) || 20;
  hsLoad(1);
});
el("hsPrev").addEventListener("click", () => hsLoad(hsView.page - 1));
el("hsNext").addEventListener("click", () => hsLoad(hsView.page + 1));
el("hsClear").addEventListener("click", async (ev) => {
  const btn = ev.currentTarget;
  // 危险操作二次确认（§13.7）：第一次点变成「确认清空」，3 秒后自己撤回
  if (btn.dataset.armed !== "1") {
    btn.dataset.armed = "1";
    btn.classList.add("armed");
    btn.textContent = "确认清空";
    setTimeout(() => {
      if (btn.dataset.armed !== "1") return;
      btn.dataset.armed = "0";
      btn.classList.remove("armed");
      btn.textContent = "清空历史";
    }, 3000);
    return;
  }
  btn.dataset.armed = "0";
  btn.classList.remove("armed");
  btn.textContent = "清空历史";
  try {
    await invoke("transfer_history_clear");
    tfNotice("历史已清空");
    await hsLoad(1);
  } catch (e) {
    tfNotice("清空失败：" + errText(e), true);
  }
});

(async function initTransfer() {
  // 模块停用时隐藏「互传」入口（S4.8）
  try {
    const tools = await invoke("list_tools");
    const t = (tools || []).find((x) => x.id === "transfer");
    if (t && !t.enabled) {
      for (const b of document.querySelectorAll('.nav-item[data-view="transfer"]')) {
        b.style.display = "none";
      }
      el("subTransfer").classList.add("hidden");
    }
  } catch (e) { /* 拿不到就照常显示 */ }

  // 托盘「局域网互传…」预选视图
  try {
    const v = await invoke("transfer_take_view");
    if (v === "transfer") setView("transfer");
  } catch (e) { /* 无预选 */ }

  try {
    await listen("transfer:event", (ev) => {
      const e = ev.payload || {};
      if (e.type === "peers") {
        tfView.peers = e.peers || [];
        renderPeers();
      } else if (e.type === "incoming") {
        // 收到请求：切到互传页，确认框是它上面的遮罩子对话框（会话 80）
        if (tfView.view !== "transfer") setView("transfer");
        setSub("receive");
      } else if (e.type === "progress") {
        upsertRow(e.view);
        tpOnProgress(e.view);
      } else if (e.type === "notice") {
        tfNotice(e.message, false);
      }
    });
    await listen("transfer:dropped", async () => {
      const paths = await invoke("transfer_take_drop");
      if (!paths || !paths.length) return;
      if (tfView.view !== "transfer") setView("transfer");
      setSub("send");
      hidePickDrop();
      addDropped(paths);
    });
    await listen("main:switch-view", (ev) => {
      if (ev.payload === "transfer") { setView("transfer"); setSub("send"); }
    });
  } catch (e) {
    tfNotice("订阅传输事件失败：" + errText(e), true);
  }

  // 传输页开场：拉一次快照（窗口中途打开也要能看到正在传的进度）
  tpLoad();
  // 选文件卡片先渲染一次空态（提示语 + 「文件：0」）
  renderPick();
  // 窗口打开前就到达的待确认请求（事件已经错过）也要弹出来
  refreshPending();
})();
