// 下载主窗口：数据全来自后端 `tasks:snapshot` 事件与 `download_snapshot` 命令。
//
// §S3.7.2 的硬要求：**增量更新**。已存在的任务行按 data-gid 复用节点，
// 只改文本与进度条宽度；只有「新出现」和「已消失」的任务才增删节点。
// 每 500ms 重建一遍 DOM 会让按钮点不中、滚动位置也丢。

const T = window.__TAURI__;
const invoke = T.core.invoke;
const listen = T.event.listen;
const el = (id) => document.getElementById(id);

// gid -> 行节点。重建窗口时为空，由 download_snapshot() 一次性填满。
const rows = new Map();
let lastError = "";

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
    acts.appendChild(b);
  }
  row.append(top, bar, meta, acts);
  return row;
}

/// 每次快照只改文本 / 宽度 / 按钮状态，不重建节点。
function updateRow(row, t) {
  const p = pct(t);
  row.querySelector(".name").textContent = t.name || "未知文件";
  row.querySelector(".pct").textContent = t.total_length ? p + "%" : "—";
  row.querySelector(".bar > i").style.width = p + "%";

  const meta = row.querySelector(".meta");
  const parts = [statusText(t.status)];
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
  const seen = new Set();
  for (const t of snap.tasks) {
    seen.add(t.gid);
    let row = rows.get(t.gid);
    if (!row) {
      row = createRow(t);
      rows.set(t.gid, row);
      el("list").appendChild(row);
    }
    updateRow(row, t);
  }
  for (const [gid, row] of rows) {
    if (!seen.has(gid)) { row.remove(); rows.delete(gid); }
  }

  el("empty").classList.toggle("hidden", snap.tasks.length > 0);
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
    case "remove": call("download_remove", { gid }); break;
    case "open":   call("download_open_dir", { gid }); break;
  }
});

el("clear").addEventListener("click", () => call("download_clear_finished"));

(async function init() {
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
  // 早先「进度卡住」是全屏 GDI 截图抓不到 WebView2 后续合成帧造成的取证误判。
})();
