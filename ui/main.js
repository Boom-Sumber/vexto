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
  box.classList.toggle("hidden", !lastError);
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
  for (const b of document.querySelectorAll("button.cat")) {
    b.classList.toggle("active", b.dataset.cat === cat);
  }
  el("title").textContent = TITLES[cat] || "全部";
  render(lastSnap);
}

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

  el("empty").classList.toggle("hidden", visible > 0);
  if (visible === 0) el("empty").innerHTML = EMPTY[filter] || EMPTY.all;

  const g = snap.global || {};
  el("global").textContent = "↓ " + fmtSpeed(g.download_speed) + "　↑ " + fmtSpeed(g.upload_speed);
  el("counts").textContent = snap.tasks.length + " 个任务 · " + (g.num_active || 0) + " 个活动";
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
  }
}

function setSub(sub) {
  tfView.sub = sub;
  for (const b of document.querySelectorAll(".sub-item[data-sub]")) {
    b.classList.toggle("active", b.dataset.sub === sub);
  }
  el("paneSend").classList.toggle("hidden", sub !== "send");
  el("paneReceive").classList.toggle("hidden", sub !== "receive");
  if (sub === "receive") {
    refreshPending();
    refreshLocalInfo();
  }
}

async function refreshLocalInfo() {
  try {
    const info = await invoke("transfer_local_info");
    tfView.receiveDir = info.receive_dir || "";
    el("localAlias").textContent = info.alias || "—";
    el("localStatus").textContent = info.enabled ? info.status : "模块已停用";
    el("localDir").textContent = info.receive_dir || "—";
    el("localDir").title = info.receive_dir || "";
  } catch (e) {
    el("localStatus").textContent = "读取本机信息失败：" + errText(e);
  }
}

function tfNotice(msg, danger) {
  const n = el("tfNotice");
  if (!msg) { n.classList.add("hidden"); return; }
  n.textContent = msg;
  n.classList.remove("hidden");
  n.classList.toggle("notice-danger", !!danger);
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

    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "btn";
    btn.textContent = "发送文件";
    btn.addEventListener("click", () => pickAndSend(p));

    d.append(av, meta, btn);
    box.appendChild(d);
  }
  // 拖放目标下拉跟着刷新
  const sel = el("tfDropTarget");
  sel.innerHTML = "";
  for (const p of tfView.peers) {
    const o = document.createElement("option");
    o.value = p.fingerprint || (p.ip + ":" + p.port);
    o.textContent = p.alias + " (" + p.ip + ")";
    sel.appendChild(o);
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

function renderPending() {
  const box = el("reqList");
  box.innerHTML = "";
  const list = tfView.pending || [];
  el("reqEmpty").classList.toggle("hidden", list.length > 0);
  for (const r of list) {
    const d = document.createElement("div");
    d.className = "tf-req";
    const names = r.files.map((f) => f.name).join("、");
    d.innerHTML =
      '<div class="rhead"><span class="who"></span><span class="num"></span></div>' +
      '<div class="rfiles"></div>' +
      '<div class="racts"><button class="btn" type="button">拒绝</button>' +
      '<button class="btn btn-primary" type="button">接受</button></div>';
    d.querySelector(".who").textContent = r.sender_alias + " (" + r.sender_ip + ")";
    d.querySelector(".num").textContent = fmtTfBytes(r.total_bytes);
    d.querySelector(".rfiles").textContent = names;
    const [rej, acc] = d.querySelectorAll("button");
    rej.addEventListener("click", () => respond(r.session_id, false));
    acc.addEventListener("click", () => respond(r.session_id, true));
    box.appendChild(d);
  }
}

async function refreshPending() {
  try {
    tfView.pending = (await invoke("transfer_pending")) || [];
    renderPending();
  } catch (e) {
    /* 确认窗是主路径，这里失败只记提示 */
    tfNotice("读取待确认请求失败：" + errText(e), true);
  }
}

async function respond(sessionId, accept) {
  try {
    await invoke("transfer_respond", { sessionId, accept });
    await refreshPending();
  } catch (e) {
    tfNotice(errText(e), true);
  }
}

function upsertRow(view) {
  const key = view.session_id + "|" + view.file_name;
  let row = tfView.rows.get(key);
  if (!row) {
    row = document.createElement("div");
    row.className = "tf-row";
    row.innerHTML =
      '<div class="line"><span class="fname"></span><span class="fstat"></span></div>' +
      '<div class="bar"><i></i></div>';
    el("tfList").appendChild(row);
    tfView.rows.set(key, row);
    el("tfEmpty").classList.add("hidden");
  }
  row.classList.toggle("done", view.status === "done");
  row.classList.toggle("failed", view.status === "failed" || view.status === "cancelled");
  row.querySelector(".fname").textContent =
    (view.direction === "send" ? "↑ " : "↓ ") + (view.file_name || "(会话)");
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

async function pickAndSend(peer) {
  try {
    const paths = await invoke("transfer_pick_files");
    if (!paths || !paths.length) return;
    await doSend(peer.fingerprint || (peer.ip + ":" + peer.port), paths, peer);
  } catch (e) {
    tfNotice("发送失败：" + errText(e), true);
  }
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

el("tfRefresh").addEventListener("click", () => { refreshPeers(); refreshPending(); });
el("tfRefreshRecv").addEventListener("click", () => { refreshPending(); refreshLocalInfo(); });
el("tfActionFile").addEventListener("click", () => el("tfPick").click());
el("tfOpenDir").addEventListener("click", () => {
  if (!tfView.receiveDir) { tfNotice("还没有读取到接收目录", true); return; }
  invoke("open_path", { path: tfView.receiveDir }).catch((e) => tfNotice(errText(e), true));
});
el("tfPick").addEventListener("click", async () => {
  if (!tfView.peers.length) { tfNotice("没有发现设备，先点「刷新」", true); return; }
  try {
    const paths = await invoke("transfer_pick_files");
    if (!paths || !paths.length) return;
    if (tfView.peers.length === 1) {
      const p = tfView.peers[0];
      await doSend(p.fingerprint || (p.ip + ":" + p.port), paths, p);
    } else {
      // 多设备：落到目标选择条，用户挑一台再发
      tfView.dropPaths.push(...paths);
      showDropBar();
    }
  } catch (e) {
    tfNotice("发送失败：" + errText(e), true);
  }
});

// 拖放文件：收集 → 选目标 → 发送
function showDropBar() {
  el("tfDropBar").classList.remove("hidden");
  el("tfDropInfo").textContent = "将发送 " + tfView.dropPaths.length + " 个文件到：";
}
el("tfDropClear").addEventListener("click", () => {
  tfView.dropPaths = [];
  el("tfDropBar").classList.add("hidden");
});
el("tfDropSend").addEventListener("click", () => {
  const target = el("tfDropTarget").value;
  if (!target) { tfNotice("没有目标设备", true); return; }
  const paths = tfView.dropPaths.slice();
  tfView.dropPaths = [];
  el("tfDropBar").classList.add("hidden");
  doSend(target, paths);
});

for (const b of document.querySelectorAll(".nav-item[data-view]")) {
  b.addEventListener("click", () => setView(b.dataset.view));
}
for (const b of document.querySelectorAll(".sub-item[data-sub]")) {
  b.addEventListener("click", () => setSub(b.dataset.sub));
}

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
        refreshPending();
      } else if (e.type === "progress") {
        upsertRow(e.view);
      } else if (e.type === "notice") {
        tfNotice(e.message, false);
      }
    });
    await listen("transfer:dropped", async () => {
      const paths = await invoke("transfer_take_drop");
      if (!paths || !paths.length) return;
      if (tfView.view !== "transfer") setView("transfer");
      setSub("send");
      tfView.dropPaths.push(...paths);
      showDropBar();
    });
    await listen("main:switch-view", (ev) => {
      if (ev.payload === "transfer") { setView("transfer"); setSub("send"); }
    });
  } catch (e) {
    tfNotice("订阅传输事件失败：" + errText(e), true);
  }
})();
