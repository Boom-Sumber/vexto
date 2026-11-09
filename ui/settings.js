// 设置页：完全由后端 `list_tools()` 的 schema 驱动（ADR-015）。
//
// **这个文件里不许出现任何一个具体工具的名字、字段名或分支。** 左侧有哪些工具、
// 每个工具几个标签页、每个标签页里有哪些字段、字段之间怎么联动（show_if），
// 全部从 Rust 侧的 `Tool::sections()` 读出来。加一个新工具时这里零改动。
//
// 保存模型（S2 任务书 §2.6.4 第 ① 条：切分区不许丢改动）：
//   switch / select / hotkey  —— 改动即存
//   text / password / number / dir —— 失焦即存（也支持回车）
// 所以页面上没有「保存」按钮。

const T = window.__TAURI__;
const invoke = T.core.invoke;
const listen = T.event.listen;
const el = (id) => document.getElementById(id);

const state = {
  tools: [],       // list_tools 的结果
  toolId: null,    // 当前选中的工具
  sections: [],    // 当前工具的 sections
  tabId: null,     // 当前标签（只有一个 section 时为 null，不画标签栏）
  values: {},      // 当前工具的字段值快照（切标签页时不丢未保存的输入）
};

// ── 小工具 ────────────────────────────────────────────────────────────────

function setStatus(msg, bad) {
  const s = el("status");
  s.textContent = msg || "";
  s.className = bad ? "bad" : "";
}

// 顶部横幅：热键被占用、保存警告都走这里，不弹窗也不写控制台（用户看不见）
function showBanner(msg) {
  const box = el("tool-status");
  if (msg) {
    box.textContent = msg;
    box.classList.remove("hidden");
  } else {
    box.textContent = "";
    box.classList.add("hidden");
  }
}

// 错误对象可能是字符串，也可能是 {message} / Tauri 的包装，统一成可读文字
function errText(e) {
  if (e == null) return "未知错误";
  if (typeof e === "string") return e;
  if (e.message) return String(e.message);
  return String(e);
}

// ── 左侧工具列表 ──────────────────────────────────────────────────────────

function renderSidebar() {
  const nav = el("tools");
  nav.textContent = "";
  for (const t of state.tools) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "tool" + (t.id === state.toolId ? " on" : "") + (t.enabled ? "" : " off");
    b.dataset.toolId = t.id;

    const dot = document.createElement("span");
    dot.className = "dot";
    const name = document.createElement("span");
    name.textContent = t.name;

    b.append(dot, name);
    b.addEventListener("click", () => selectTool(t.id));
    nav.append(b);
  }
}

// ── 选中一个工具 ──────────────────────────────────────────────────────────

async function selectTool(id, opts = {}) {
  if (!id) return;
  state.toolId = id;
  const tool = state.tools.find((t) => t.id === id);
  if (!tool) return;

  // 切工具时把上一个工具的横幅清掉，避免张冠李戴
  if (!opts.keepBanner) showBanner("");

  state.sections = tool.sections || [];
  // 只有一个 section 时不画标签栏，直接渲染
  state.tabId = opts.tabId || (state.sections.length === 1 ? state.sections[0].id : state.sections[0]?.id);

  el("tool-name").textContent = tool.name;
  el("tool-desc").textContent = tool.desc || "";
  renderStatusLine(tool);
  renderEnable(tool);
  renderSidebar();
  renderTabs();

  // 字段值从后端拿：只返回该工具声明的 key，不会把别的工具的值漏出来
  try {
    const res = await invoke("get_tool_config", { toolId: id });
    state.values = (res && res.values) || {};
  } catch (e) {
    setStatus("读取配置失败：" + errText(e), true);
    state.values = {};
  }

  renderFields();

  // 翻译工具的热键注册失败原因要显示出来
  await refreshHotkeyError();
}

function renderStatusLine(tool) {
  const p = el("tool-status-line");
  p.textContent = tool.status_line || "";
  // 状态行的颜色只是提示，判据仍然是 enabled
  p.className = "status-line" + (tool.enabled ? "" : " off");
}

function renderEnable(tool) {
  const cb = el("tool-enabled");
  cb.checked = !!tool.enabled;
  // 不在这里隐藏：后端可能拒绝（「通用」是配置分组不能停用），
  // 拒绝时勾选状态回滚并显示原因，比"看不见这个开关"更诚实。
  el("enable-wrap").classList.remove("hidden");
  cb.onchange = async () => {
    const want = cb.checked;
    cb.disabled = true;
    try {
      const res = await invoke("set_tool_enabled", { toolId: tool.id, enabled: want });
      if (res && res.ok) {
        setStatus(res.message || "已保存");
        await reloadTools();
        await selectTool(tool.id, { keepBanner: true });
      } else {
        cb.checked = !want;   // 回滚
        showBanner((res && res.message) || "切换失败");
        setStatus("", false);
      }
    } catch (e) {
      cb.checked = !want;
      showBanner("切换失败：" + errText(e));
    } finally {
      cb.disabled = false;
    }
  };
}

function renderTabs() {
  const bar = el("tabs");
  bar.textContent = "";
  if (state.sections.length <= 1) {
    bar.classList.add("hidden");
    return;
  }
  bar.classList.remove("hidden");
  for (const s of state.sections) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "tab" + (s.id === state.tabId ? " on" : "");
    b.textContent = s.name;
    b.addEventListener("click", () => {
      state.tabId = s.id;
      renderTabs();
      renderFields();
    });
    bar.append(b);
  }
}

// ── 字段渲染（9 种类型）──────────────────────────────────────────────────

function currentSection() {
  return state.sections.find((s) => s.id === state.tabId) || state.sections[0] || null;
}

function renderFields() {
  const box = el("fields");
  box.textContent = "";
  const sec = currentSection();
  if (!sec) return;
  for (const f of sec.fields) {
    const node = renderField(f);
    if (node) box.append(node);
  }
  applyShowIf();
}

function val(key) {
  return state.values[key];
}

function renderField(f) {
  switch (f.type) {
    case "text":
      return fieldInput(f, "text");
    case "password":
      return fieldInput(f, "password");
    case "dir":
      // S2 先用文本框填完整路径，原生目录选择器留到后续会话（用户 m02150 定）。
      return fieldInput(f, "text", f.placeholder || "例如 D:\\Downloads");
    case "number":
      return fieldNumber(f);
    case "switch":
      return fieldSwitch(f);
    case "select":
      return fieldSelect(f);
    case "hotkey":
      return fieldHotkey(f);
    case "button":
      return fieldButton(f);
    case "note":
      return fieldNote(f);
    default:
      // 后端加了新类型而前端还没跟上：明确报出来，不要静默吞掉
      return noteNode("未知字段类型「" + f.type + "」，请更新设置页");
  }
}

function wrapField(f, extraClass) {
  const w = document.createElement("div");
  w.className = "field" + (extraClass ? " " + extraClass : "");
  w.dataset.key = f.key;
  if (f.show_if) w.dataset.showIf = JSON.stringify(f.show_if);
  return w;
}

function labelNode(text) {
  const l = document.createElement("span");
  l.className = "field-label";
  l.textContent = text;
  return l;
}

function noteNode(text) {
  const d = document.createElement("div");
  d.className = "field note";
  d.textContent = text;
  return d;
}

function fieldNote(f) {
  const d = noteNode(f.text || "");
  // 说明文字也可能联动（「引擎=百度才显示百度控制台在哪」这类）。
  // note 没走 wrapField（它不需要 key/label），所以这里单独把 show_if 带上，
  // 否则切到别的引擎后这行说明会一直挂着。
  if (f.show_if) d.dataset.showIf = JSON.stringify(f.show_if);
  return d;
}

function fieldInput(f, inputType) {
  const w = wrapField(f);
  const l = labelNode(f.label || "");
  const inp = document.createElement("input");
  inp.type = inputType;
  inp.value = val(f.key) == null ? "" : String(val(f.key));
  if (f.placeholder) inp.placeholder = f.placeholder;
  inp.spellcheck = false;
  inp.autocomplete = "off";

  // 失焦保存；回车也保存（顺手把焦点放掉）
  inp.addEventListener("blur", () => commit(f.key, inp.value.trim()));
  inp.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      inp.blur();
    }
  });

  w.append(l, inp);
  return w;
}

function fieldNumber(f) {
  const w = wrapField(f);
  const l = labelNode(f.label || "");

  const row = document.createElement("div");
  row.className = "num-row" + (f.unit ? " has-unit" : "");

  const inp = document.createElement("input");
  inp.type = "number";
  inp.value = val(f.key) == null ? "" : String(val(f.key));
  if (typeof f.min === "number") inp.min = String(f.min);
  if (typeof f.max === "number") inp.max = String(f.max);

  function saveNumber() {
    let n = Number(inp.value);
    if (!Number.isFinite(n)) {
      // 清空或乱填：回退到当前值，不保存非法数
      inp.value = val(f.key) == null ? "" : String(val(f.key));
      return;
    }
    // 越界时夹到范围内再保存，并回显夹取后的值（用户能看到发生了什么）
    if (typeof f.min === "number" && n < f.min) n = f.min;
    if (typeof f.max === "number" && n > f.max) n = f.max;
    inp.value = String(n);
    commit(f.key, n);
  }

  inp.addEventListener("blur", saveNumber);

  row.append(inp);
  if (f.unit) {
    const u = document.createElement("span");
    u.className = "num-unit";
    u.textContent = f.unit;
    row.append(u);
  }

  // 系统 spinner 是白色的（用户反馈），改成 base.css 里的自绘步进。
  // 点箭头等于改值 + 立刻保存，和手动输入后失焦同一条路径。
  window.NumField.enhance(inp, (n) => {
    inp.value = String(n);
    commit(f.key, n);
  });

  w.append(l, row);
  return w;
}

function fieldSwitch(f) {
  const w = wrapField(f, "switch");
  const inp = document.createElement("input");
  inp.type = "checkbox";
  inp.checked = !!val(f.key);
  const l = labelNode(f.label || "");
  // 点标题也能切换
  l.addEventListener("click", () => {
    inp.checked = !inp.checked;
    inp.dispatchEvent(new Event("change"));
  });
  inp.addEventListener("change", () => commit(f.key, inp.checked));
  w.append(inp, l);
  return w;
}

function fieldSelect(f) {
  const w = wrapField(f);
  const l = labelNode(f.label || "");
  const sel = document.createElement("select");
  const cur = val(f.key);
  for (const opt of f.options || []) {
    // options 是 [[value, label], ...] 的二元组数组
    const [v, txt] = opt;
    const o = document.createElement("option");
    o.value = v;
    o.textContent = txt;
    if (String(cur) === String(v)) o.selected = true;
    sel.append(o);
  }
  sel.addEventListener("change", () => commit(f.key, sel.value));
  w.append(l, sel);
  return w;
}

// ── 热键捕获状态机（规格见 docs/stages/S2-配置与设置中心.md §2.6.3 / ADR-018）──
const MODS = ["Ctrl", "Shift", "Alt"];              // 白名单，顺序即归一化顺序
const MOD_KEYS = { Control: "Ctrl", Shift: "Shift", Alt: "Alt" };
const MAIN_RE = /^(Key[A-Z]|Digit[0-9]|F([1-9]|1[0-2]))$/;   // 主键白名单

let capture = null;   // { el, prev, mods: [], key }

function beginCapture(input, key) {
  if (capture) return;
  capture = { el: input, prev: input.value, mods: [], key };
  input.value = "";
  input.placeholder = "请按下组合键…";
  input.classList.add("capturing");
  document.addEventListener("keydown", onKeyDown, true);
  input.addEventListener("blur", cancelCapture, { once: true });
}

function onKeyDown(e) {
  if (!capture) return;
  e.preventDefault();
  e.stopPropagation();

  if (e.key === "Escape") return cancelCapture();

  const mod = MOD_KEYS[e.key];
  if (mod) {
    // 第三个修饰键：静默取最后两个（Ctrl+Shift+Alt+T 是常见误操作，报错只会烦人）
    if (!capture.mods.includes(mod)) capture.mods.push(mod);
    if (capture.mods.length > 2) capture.mods = capture.mods.slice(-2);
    capture.el.value = normalizeMods(capture.mods) + "+";
    return;
  }

  // 主键：必须在白名单，且至少有一个修饰键
  if (!MAIN_RE.test(e.code)) return;          // 方向键、符号键、F13+ 一律忽略，不退出捕获
  if (capture.mods.length === 0) {
    capture.el.placeholder = "必须先按 Ctrl / Shift / Alt";
    return;
  }
  const main = e.code.startsWith("Key") ? e.code.slice(3)
             : e.code.startsWith("Digit") ? e.code.slice(5)
             : e.code;                        // F1..F12 原样
  capture.el.value = normalizeMods(capture.mods) + "+" + main;
  endCapture(true);
}

// item 顺序必须与 MODS 一致：Ctrl → Shift → Alt
function normalizeMods(mods) {
  return MODS.filter((m) => mods.includes(m)).join("+");
}

function endCapture(save) {
  if (!capture) return;
  const { el: input, prev, key } = capture;
  document.removeEventListener("keydown", onKeyDown, true);
  input.classList.remove("capturing");
  input.placeholder = "点击设置";
  if (!save) input.value = prev;
  capture = null;
  // 捕获完成即保存（后端仍会重新校验一遍，前端只是省一次点击）
  if (save) commit(key, input.value);
}

function cancelCapture() { endCapture(false); }

function fieldHotkey(f) {
  const w = wrapField(f);
  const l = labelNode(f.label || "");
  const inp = document.createElement("input");
  inp.type = "text";
  inp.className = "capture";
  inp.readOnly = true;
  inp.spellcheck = false;
  inp.value = val(f.key) == null ? "" : String(val(f.key));
  inp.placeholder = "点击设置";
  inp.addEventListener("click", () => beginCapture(inp, f.key));
  inp.addEventListener("focus", () => beginCapture(inp, f.key));
  w.append(l, inp);
  return w;
}

function fieldButton(f) {
  const w = wrapField(f, "action");
  const b = document.createElement("button");
  b.type = "button";
  b.className = "btn";
  b.textContent = f.label || f.key;
  b.addEventListener("click", () => runAction(f, b, w));
  w.append(b);
  return w;
}

// 动作型字段：key 就是后端的命令名，前端不认识具体业务
async function runAction(f, btn, wrapEl) {
  btn.disabled = true;
  const old = wrapEl.querySelector(".result");
  if (old) old.remove();
  try {
    const res = await invoke(f.key);
    showBanner("");
    renderResult(wrapEl, res);
  } catch (e) {
    showBanner(errText(e));
  } finally {
    btn.disabled = false;
  }
}

// 后端的动作命令返回 {ok, message, ...}，统一渲染成结果框
function renderResult(wrapEl, res) {
  const box = document.createElement("div");
  const ok = !res || res.ok !== false;
  box.className = "result " + (ok ? "ok" : "bad");
  let text = (res && res.message) || (ok ? "完成" : "失败");
  if (res && res.ok && typeof res.ms === "number") {
    text += "\n语向：" + (res.from || "?") + " → " + (res.to || "?") + "，耗时 " + res.ms + " ms";
  }
  box.textContent = text;
  wrapEl.append(box);
  if (res && Array.isArray(res.warnings) && res.warnings.length) {
    showBanner(res.warnings.join("\n"));
  }
}

// ── show_if 联动 ─────────────────────────────────────────────────────────

function applyShowIf() {
  const nodes = el("fields").querySelectorAll("[data-show-if]");
  for (const n of nodes) {
    let cond;
    try {
      cond = JSON.parse(n.dataset.showIf);
    } catch {
      continue;
    }
    const shown = String(val(cond.key)) === String(cond.equals);
    n.classList.toggle("hidden", !shown);
  }
}

// ── 保存 ─────────────────────────────────────────────────────────────────

// 只发改动的那一个 key：后端是「读现有配置 → 覆盖给定键 → 写盘」，
// 发全量反而容易把别处的值覆盖掉（S2a 阶段踩过这个坑）。
async function commit(key, value) {
  state.values[key] = value;
  applyShowIf();          // 立刻重算联动，不等保存结果
  setStatus("保存中…");
  try {
    const res = await invoke("save_tool_config", {
      toolId: state.toolId,
      values: { [key]: value },
    });
    if (res && res.ok) {
      setStatus(res.message || "已保存");
      // 警告照旧显示，但有 warning 时不要被"已保存"盖掉
      if (Array.isArray(res.warnings) && res.warnings.length) {
        showBanner(res.warnings.join("\n"));
      }
      // 后端会把热键归一化、状态摘要也会变，所以回读一次
      await refreshAfterSave();
    } else {
      setStatus((res && res.message) || "保存失败", true);
      showBanner((res && res.message) || "保存失败");
      // 保存失败时把值退回后端真实值，避免界面骗人
      await reloadValues();
    }
  } catch (e) {
    setStatus("保存失败：" + errText(e), true);
    showBanner("保存失败：" + errText(e));
    await reloadValues();
  }
}

async function reloadValues() {
  try {
    const res = await invoke("get_tool_config", { toolId: state.toolId });
    state.values = (res && res.values) || {};
    renderFields();
  } catch {
    /* 读不回来就保持现状，至少不崩 */
  }
}

// 保存后刷新状态摘要与工具列表（启停状态、状态行都会变）
async function refreshAfterSave() {
  await reloadTools();
  const tool = state.tools.find((t) => t.id === state.toolId);
  if (tool) {
    renderStatusLine(tool);
    renderEnable(tool);
    renderSidebar();
    // 热键变了要重新注册，结果在 hotkey_error 里
    await refreshHotkeyError();
  }
}

async function reloadTools() {
  try {
    state.tools = await invoke("list_tools");
  } catch (e) {
    setStatus("读取工具列表失败：" + errText(e), true);
  }
}

// 热键注册失败原因：后端把它存在一个全局里。
// 热键现在跨两个工具（翻译的划词/截图、下载的唤起），没法归到某一个工具的
// 状态行里，所以这里按「全局告警」处理 —— 任何工具页都显示，直到下次保存成功。
async function refreshHotkeyError() {
  try {
    const msg = await invoke("hotkey_error");
    if (msg) showBanner(msg);
  } catch {
    /* 没有就不显示 */
  }
}

// ── 启动 ─────────────────────────────────────────────────────────────────

async function boot() {
  await reloadTools();

  // 托盘点了某个工具、或从别处跳过来：定位到它
  let pending = null;
  try {
    pending = await invoke("take_pending_tool");
  } catch {
    /* 没有就算了 */
  }

  const first = pending && state.tools.some((t) => t.id === pending)
    ? pending
    : (state.tools[0] && state.tools[0].id);
  await selectTool(first);

  // 设置窗口已经开着时再跳转：走事件而不是重载页面
  await listen("settings-goto", async (ev) => {
    const id = ev && ev.payload;
    if (id && state.tools.some((t) => t.id === id)) {
      await selectTool(id, { keepBanner: true });
    }
  });
}

window.addEventListener("DOMContentLoaded", boot);
