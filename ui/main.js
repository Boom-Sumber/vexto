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
})();
