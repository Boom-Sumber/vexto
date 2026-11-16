// 设置页：完全由后端 `list_tools()` 的 schema 驱动（ADR-015）。
//
// **这个文件里不许出现任何一个具体工具的名字、字段名或分支。** 左侧有哪些工具、
// 每个工具几个标签页、每个标签页里有哪些字段、字段之间怎么联动（show_if），
// 全部从 Rust 侧的 `Tool::sections()` 读出来。加一个新工具时这里零改动。
//
// 保存模型（会话 28 起改成**手动保存**）：
//   改任何字段只是进 `state.pending`，底栏出现「保存 / 放弃」；
//   点保存才发一次 `save_tool_config`（多个 key 一起交），点放弃就丢掉重读。
//   有未保存改动时，切工具 / 关窗口都会先弹确认框——那是用户自己改的东西，
//   不能因为点错一下就没了。
//
// 为什么改：之前「改一下就存」在改到一半时（比如刚改了限速还没填连接数）
// 也会立刻落盘，用户没有一个「这批改动我想反悔」的出口。
//
// **会话 31 的两处有意例外**（都只服务于「引擎配置」这一类，别在别处照抄）：
//   ① `engineChoices()` 从两张列表算出动态下拉的候选；后端 schema 只给一个
//      数据源名 `engines`，没有把「哪两张列表」也序列化下来。
//   ② `fieldList()` 删除当前实例时顺位到第一项——要先改 `active_engine`，
//      不这么做下拉会短暂指到一个已不存在的 id。
// 其它工具、其它字段一律仍走 schema，不认识这些键也就不会被影响。见 ADR-031。
//
// **会话 51（UI v2）**：字段包装层从「两栏 grid」换成 demo 的卡片行
// （`.card > .setting-row > .meta(标题+描述) + .ctrl(控件)`），描述直接取
// 字段的 `help`（原来是「?」悬浮）。分发 / 取值 / 联动 / 保存 / 热键状态机
// / 弹窗全部原样；「?」图标只保留给没有标题行的按钮字段。

const T = window.__TAURI__;
const invoke = T.core.invoke;
const listen = T.event.listen;
const el = (id) => document.getElementById(id);

const state = {
  tools: [],       // list_tools 的结果
  toolId: null,    // 当前选中的工具
  sections: [],    // 当前工具的 sections
  tabId: null,     // 当前标签（只有一个 section 时为 null，不画标签栏）
  values: {},      // 后端读回来的值
  pending: {},     // 还没保存的改动（key → value），优先于 values
};

// 页面上所有「动态下拉」的 select 节点（目前只有翻译模式一个）。
// 候选来自同页的两张列表，改名 / 增删后要原地刷新，不能整页重画（会丢焦点）。
let engineSelectNodes = [];

// ── 小工具 ────────────────────────────────────────────────────────────────

function setStatus(msg, kind) {
  const s = el("status");
  s.textContent = msg || "";
  s.className = kind === true ? "bad" : (typeof kind === "string" ? kind : "");
  clearTimeout(statusTimer);
  // 页脚状态自动消失；「有 N 项未保存」是常驻状态，交给 renderDirty 管。
  if (msg && kind !== "dirty") {
    statusTimer = setTimeout(() => {
      if (pendingCount() > 0) return;
      s.textContent = "";
      s.className = "";
    }, 4000);
  }
}
let statusTimer = null;

/// 右下角轻提示：操作类反馈统一走这里，3 秒消失（错误 6 秒）。
let toastTimer = null;
function showToast(msg, kind) {
  if (!msg) return;
  let box = el("toast");
  if (!box) {
    box = document.createElement("div");
    box.id = "toast";
    box.setAttribute("role", "status");
    box.setAttribute("aria-live", "polite");
    document.body.append(box);
  }
  const bad = kind === "bad";
  box.className = "toast on" + (bad ? " bad" : "");
  box.innerHTML =
    Icons.svg(bad ? "alert" : "info", 16) + "<span></span>";
  box.querySelector("span").textContent = msg;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => box.classList.remove("on"), bad ? 6000 : 3000);
}

// 顶部横幅：热键被占用、保存警告都走这里，不弹窗也不写控制台（用户看不见）。
// 与 toast 一样倒计时消失；真正需要长期可见的问题会在状态行 / 字段值里保留。
let bannerTimer = null;
function showBanner(msg) {
  const box = el("tool-status");
  clearTimeout(bannerTimer);
  if (msg) {
    box.textContent = msg;
    box.classList.remove("hidden");
    bannerTimer = setTimeout(() => {
      box.textContent = "";
      box.classList.add("hidden");
    }, 8000);
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

    // 图标来自 list_tools 的 icon 字段（纯展示，ADR-015 口径不变：
    // 前端仍不认识任何工具名，没给图标就退回圆点）。
    let lead;
    if (t.icon && Icons.has(t.icon)) {
      lead = document.createElement("span");
      lead.className = "t-ico";
      lead.innerHTML = Icons.svg(t.icon, 20);
    } else {
      lead = document.createElement("span");
      lead.className = "dot";
    }
    const name = document.createElement("span");
    name.className = "t-name";
    name.textContent = t.name;

    b.append(lead, name);
    b.addEventListener("click", () => selectTool(t.id));
    nav.append(b);
  }
}

// ── 选中一个工具 ──────────────────────────────────────────────────────────

async function selectTool(id, opts = {}) {
  if (!id) return;
  // 有未保存改动又切到别的工具：先问。点同一个工具不算切换，pending 留着。
  if (!opts.skipGuard && pendingCount() > 0 && id !== state.toolId) {
    showDirtyModal({ kind: "switch", toolId: id });
    return;
  }
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
  // 颜色语义由后端 status_kind 给（ok 绿 / warn 黄 / error 红，会话 52）；
  // 停用的工具一律灰——「已停用」不是错误。
  let kind = "off";
  if (tool.enabled !== false) {
    kind = tool.status_kind === "error" ? "error"
         : tool.status_kind === "warn" ? "warn"
         : "";
  }
  p.className = "status-line" + (kind ? " " + kind : "");
}

function renderEnable(tool) {
  const cb = el("tool-enabled");
  // 「通用」是配置容器、「关于」是只读信息，后端 switchable=false 时整块不画。
  // 以前是画出来、点了回滚再弹原因；用户明确指出这两个页面不该有启用选项。
  const canSwitch = tool.switchable !== false;
  el("enable-wrap").classList.toggle("hidden", !canSwitch);
  cb.onchange = null;
  if (!canSwitch) return;
  cb.checked = !!tool.enabled;
  cb.onchange = async () => {
    const want = cb.checked;
    cb.disabled = true;
    try {
      const res = await invoke("set_tool_enabled", { toolId: tool.id, enabled: want });
      if (res && res.ok) {
        setStatus(res.message || "已保存");
        // 启动成功就收掉上一次失败留下的红横幅（会话 52：错误不许赖着不走）。
        // 若热键注册等仍有新问题，下面的 selectTool → refreshHotkeyError 会再挂上来。
        showBanner("");
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
  // 切工具 / 切页签时字段 DOM 会被换掉，悬停的 ? 收不到 mouseout；
  // 不主动收起的话上一页的说明会一直挂在屏幕上。
  hideHelp();
  box.textContent = "";
  // 字段 DOM 全换了，旧的下拉节点不再有效（isConnected=false），别留着
  engineSelectNodes = [];
  const sec = currentSection();
  if (!sec) return;

  // v2 卡片式分组（demo 语言）：group 标题断开当前卡片；普通字段进卡片行；
  // list 自成卡片（.list-row，不嵌进分组卡）；note 跟随所在卡片，无卡片时裸排。
  let card = null;
  const ensureCard = () => {
    if (!card) {
      card = document.createElement("div");
      card.className = "card";
      box.append(card);
    }
    return card;
  };

  for (const f of sec.fields) {
    if (f.type === "group") {
      card = null;
      box.append(fieldGroup(f));
      continue;
    }
    if (f.type === "list") {
      card = null;
      const node = renderField(f);
      if (node) box.append(node);
      continue;
    }
    const node = renderField(f);
    if (!node) continue;
    if (f.type === "note" && !card) {
      box.append(node);
      continue;
    }
    ensureCard().append(node);
  }
  applyShowIf();
}

/// 字段当前该显示什么：**未保存的改动优先**。
/// 切页签、切工具回来时靠它保住用户刚填的值（不回退成后端旧值）。
function val(key) {
  if (Object.prototype.hasOwnProperty.call(state.pending, key)) {
    return state.pending[key];
  }
  return state.values[key];
}

function pendingCount() {
  return Object.keys(state.pending).length;
}

function renderField(f) {
  switch (f.type) {
    case "text":
      return fieldInput(f, "text");
    case "password":
      return fieldInput(f, "password");
    case "template":
      return fieldTemplate(f);
    case "dir":
      // S2 先用文本框填完整路径，原生目录选择器留到后续会话（用户 m02150 定）。
      return fieldInput(f, "text", f.placeholder || "例如 D:\\Downloads");
    case "number":
      return fieldNumber(f);
    case "switch":
      return fieldSwitch(f);
    case "select":
      return fieldSelect(f);
    case "color":
      return fieldColor(f);
    case "hotkey":
      return fieldHotkey(f);
    case "button":
      return fieldButton(f);
    case "button_row":
      return fieldButtonRow(f);
    case "note":
      return fieldNote(f);
    case "group":
      return fieldGroup(f);
    case "list":
      return fieldList(f);
    default:
      // 后端加了新类型而前端还没跟上：明确报出来，不要静默吞掉
      return noteNode("未知字段类型「" + f.type + "」，请更新设置页");
  }
}

function wrapField(f, extraClass) {
  const w = document.createElement("div");
  w.className = "field" + (extraClass ? " " + extraClass : "") + (f.wide ? " wide" : "");
  w.dataset.key = f.key;
  if (f.show_if) w.dataset.showIf = JSON.stringify(f.show_if);
  return w;
}

/// 卡片行的左栏：标题 + 描述。描述取字段的 `help`——原来是「?」悬浮说明，
/// v2 升级为可见文案；没有 help 就只有标题。
function metaNode(f) {
  const m = document.createElement("div");
  m.className = "meta";
  const t = document.createElement("div");
  t.className = "t";
  t.textContent = f.label || "";
  m.append(t);
  if (f.help) {
    const d = document.createElement("div");
    d.className = "d";
    d.textContent = f.help;
    m.append(d);
  }
  return m;
}

/// 控件列：一行卡片里右侧的所有东西（输入框、预设排、结果框…）。
function ctrlNode() {
  const c = document.createElement("div");
  c.className = "ctrl";
  return c;
}

/// 把一个字段包成 demo 的卡片行，返回 [wrapper, ctrl]。
/// `show_if` 仍挂在 wrapper 上（applyShowIf 按 data-show-if 显隐整行）。
function fieldRow(f, extraClass) {
  const w = wrapField(f, extraClass);
  const row = document.createElement("div");
  row.className = "setting-row";
  const ctrl = ctrlNode();
  row.append(metaNode(f), ctrl);
  w.append(row);
  return [w, ctrl];
}

/// 「?」图标：鼠标悬停 / 键盘聚焦时在全局浮层里显示说明。
/// 说明不再单独占一行（用户要求设置页不放说明文本）。
function helpIcon(text) {
  const h = document.createElement("span");
  h.className = "field-help";
  h.tabIndex = 0;
  h.setAttribute("role", "note");
  h.setAttribute("aria-label", "说明");
  h.dataset.help = text;
  h.innerHTML = Icons.svg("help", 14);
  // 开关行的标题点击会切换开关；点问号只弹说明，不要顺手切换。
  h.addEventListener("click", (e) => e.stopPropagation());
  return h;
}

let helpTipEl = null;
function ensureHelpTip() {
  if (!helpTipEl) {
    helpTipEl = document.createElement("div");
    helpTipEl.id = "help-tip";
    document.body.append(helpTipEl);
  }
  return helpTipEl;
}

function showHelp(el) {
  const text = el.dataset.help || "";
  if (!text) return;
  const tip = ensureHelpTip();
  tip.textContent = text;
  tip.classList.add("on");
  const r = el.getBoundingClientRect();
  const tw = tip.offsetWidth;
  const th = tip.offsetHeight;
  let left = Math.max(8, Math.min(r.left, window.innerWidth - tw - 8));
  let top = r.bottom + 6;
  if (top + th > window.innerHeight - 8) top = r.top - th - 6;
  tip.style.left = left + "px";
  tip.style.top = top + "px";
}

function hideHelp() {
  if (helpTipEl) helpTipEl.classList.remove("on");
}

document.addEventListener("mouseover", (e) => {
  const h = e.target.closest?.(".field-help");
  if (h) showHelp(h);
});
document.addEventListener("mouseout", (e) => {
  const h = e.target.closest?.(".field-help");
  if (h) hideHelp();
});
document.addEventListener("focusin", (e) => {
  const h = e.target.closest?.(".field-help");
  if (h) showHelp(h);
});
document.addEventListener("focusout", (e) => {
  const h = e.target.closest?.(".field-help");
  if (h) hideHelp();
});
window.addEventListener("scroll", hideHelp, true);

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

/// 分组标题：页签内部再分层（下载路径 / 速度与并发 / 通知…）。
///
/// 它没有 key、没有值，也不参与 show_if —— 一个纯版式元素。
/// 放在 `sections()` 里而不是前端写死，是为了让「这页长什么样」仍然由工具自己描述。
function fieldGroup(f) {
  const d = document.createElement("div");
  d.className = "group-title";
  d.textContent = f.title || "";
  return d;
}

// ── 可重复的一组配置（list）────────────────────────────────────────────
//
// 卡片永远是「一行摘要 + 展开箭头 + 删除」：点箭头摊开详细配置，就地可改；
// 新建走弹窗（`add_dialog`）。摘要取后端给的 `summary`，没给就用前两个字段。
//
// 输入时不重画（否则光标会跳），只有增删、切换下拉才动 DOM。
// 列表变了要 `renderFields()`：上面「翻译模式」的下拉是从这些列表算出来的。

// ── 引擎实例的测试状态（会话 33）────────────────────────────────────────
// 只活在内存里：测试是临时的，不该写进 config.json，也不该在下次打开设置页时
// 还亮着上一次的绿灯。key = `${listKey}:${item.id}`；sig 是测试那一刻的配置
// 指纹，配置后来被改过就对不上，自动回到「未测试」灰球。
const profileStatus = new Map();
const STATUS_TEXT = { ok: "测试通过", bad: "测试失败", idle: "未测试" };

function setProfileStatus(listKey, item, status) {
  profileStatus.set(`${listKey}:${item.id}`, { status, sig: stableJson(item) });
}
function getProfileStatus(listKey, item) {
  const s = profileStatus.get(`${listKey}:${item.id}`);
  return s && s.sig === stableJson(item) ? s.status : "idle";
}
function markStatus(dot, status) {
  dot.dataset.status = status;
  dot.title = STATUS_TEXT[status] || STATUS_TEXT.idle;
}

/// 实测一套引擎配置。卡片里刚改的值、弹窗里还没保存的草稿都能测：
/// 后端用一份临时配置发一句翻译，不写盘、也不动当前正在用的引擎。
/// 呈现方式交给调用方：卡片只点状态球，弹窗只写一句结论，都不再摊开
/// 译文 / 语向 / 耗时那一长串文本。
async function testProfile(action, item, onResult, btn) {
  const label = btn.textContent;
  btn.disabled = true;
  btn.textContent = "测试中…";
  let res;
  try {
    res = await invoke(action, { profile: item });
  } catch (e) {
    res = { ok: false, message: errText(e) };
  } finally {
    btn.disabled = false;
    btn.textContent = label;
  }
  onResult(res);
}

/// 弹窗里的结论框：绿框「测试通过」/ 红框「测试失败」，不显示任何细节。
function renderSimpleResult(host, status) {
  const old = host.querySelector(".result");
  if (old) old.remove();
  const box = document.createElement("div");
  box.className = "result " + (status === "ok" ? "ok" : "bad");
  box.textContent = STATUS_TEXT[status] || STATUS_TEXT.bad;
  host.append(box);
}

function newItemId() {
  return "m" + Date.now().toString(36) + Math.random().toString(36).slice(2, 8);
}

// 一条摘要：select 显示选项文字、switch 显示中文，其余原样。
// 返回数组——卡片行把第一段当标题、其余段当描述（v2 的两行结构）。
function summaryParts(f, it) {
  const keys = (f.summary && f.summary.length
    ? f.summary
    : (f.item_fields || []).slice(0, 2).map((x) => x.key));
  const parts = [];
  for (const k of keys) {
    const fld = (f.item_fields || []).find((x) => x.key === k);
    let v = it[k];
    if (fld && fld.type === "select") {
      const hit = (fld.options || []).find((o) => String(o[0]) === String(v));
      if (hit) v = hit[1];
    } else if (fld && fld.type === "switch") {
      v = k === "use_llm" ? (v ? "大模型" : "机翻") : v ? "开" : "关";
    }
    if (v != null && String(v).trim() !== "") parts.push(String(v));
  }
  return parts;
}

// 项内联动：只看这一项自己的字段，不跨项、不读全局 pending
function itemShowIf(fld, it) {
  if (!fld.show_if) return true;
  const list = Array.isArray(fld.show_if.equals)
    ? fld.show_if.equals
    : [fld.show_if.equals];
  return list.some((e) => String(e) === String(it[fld.show_if.key] ?? ""));
}

// 一个控件（详情区与弹窗共用）：text / password / select / switch。
// switch 返回的是整个小号 pill 开关（label.switch-pill.sm，会话 52）——
// 列表项里的开关比页面级开关小一号，调用方直接 append，不再自己包 label。
function itemControl(fld, value, onChange) {
  if (fld.type === "switch") {
    const lab = document.createElement("label");
    lab.className = "switch-pill sm";
    const cb = document.createElement("input");
    cb.type = "checkbox";
    cb.checked = !!value;
    const track = document.createElement("span");
    track.className = "track";
    const thumb = document.createElement("span");
    thumb.className = "thumb";
    lab.append(cb, track, thumb);
    cb.addEventListener("change", () => onChange(cb.checked));
    return lab;
  }
  if (fld.type === "select") {
    const sel = document.createElement("select");
    for (const [v, txt] of fld.options || []) {
      const o = document.createElement("option");
      o.value = v;
      o.textContent = txt;
      if (String(value) === String(v)) o.selected = true;
      sel.append(o);
    }
    sel.addEventListener("change", () => onChange(sel.value));
    return sel;
  }
  const inp = document.createElement("input");
  inp.className = "input";
  inp.type = fld.type === "password" ? "password" : "text";
  inp.spellcheck = false;
  inp.value = value == null ? "" : String(value);
  if (fld.placeholder) inp.placeholder = fld.placeholder;
  inp.addEventListener("input", () => onChange(inp.value));
  return inp;
}

function fieldList(f) {
  const w = wrapField(f, "list");
  // 列表字段的标签画成小节标题（demo 的 .subhead）；自成一组卡片，不进分组卡。
  const sh = document.createElement("div");
  sh.className = "subhead";
  sh.textContent = f.label || "";
  w.append(sh);
  if (f.help) {
    const d = document.createElement("div");
    d.className = "subhead-desc";
    d.textContent = f.help;
    w.append(d);
  }

  const body = document.createElement("div");
  body.className = "list-body";
  w.append(body);

  const itemFields = f.item_fields || [];
  const sums = new Map();    // id → 摘要节点
  const details = new Map();  // id → 详情容器
  const dots = new Map();     // id → 卡片头的状态球（会话 33）

  function readItems() {
    const v = val(f.key);
    return Array.isArray(v) ? JSON.parse(JSON.stringify(v)) : [];
  }

  function patch(id, key, value) {
    const next = readItems();
    const it = next.find((x) => x.id === id);
    if (!it) return;
    it[key] = value;
    commit(f.key, next);
    refreshSummary(id);
    // 测过之后又把配置改了，绿灯 / 红灯就不再代表当前值 → 回到「未测试」。
    // 指纹机制保证下次重画也是灰的，这里只负责立刻改眼前这颗球。
    const dot = dots.get(id);
    if (dot) {
      profileStatus.delete(`${f.key}:${id}`);
      markStatus(dot, "idle");
    }
  }

  function refreshSummary(id) {
    const it = readItems().find((x) => x.id === id);
    const n = sums.get(id);
    if (!it || !n) return;
    const parts = summaryParts(f, it);
    n.nm.textContent = parts[0] || "未命名";
    if (n.de) n.de.textContent = parts.slice(1).join(" · ");
  }

  function fillDetail(box, it) {
    box.textContent = "";
    for (const fld of itemFields) {
      if (!itemShowIf(fld, it)) continue;
      const row = document.createElement("div");
      row.className = "li-row";

      const lab = document.createElement("span");
      lab.className = "li-label";
      lab.textContent = fld.label || fld.key;
      row.append(lab);

      const ctrl = itemControl(fld, it[fld.key], (v) => {
        patch(it.id, fld.key, v);
        // 切厂商要让另一家的凭据行出现 / 消失
        if (fld.type === "select") {
          const fresh = readItems().find((x) => x.id === it.id);
          if (fresh) fillDetail(box, fresh);
        }
      });
      // switch 时 ctrl 本身就是 label.switch-pill，直接进网格
      row.append(ctrl);
      box.append(row);
    }
  }

  function card(it) {
    const c = document.createElement("div");
    c.className = "list-item";

    const head = document.createElement("div");
    head.className = "li-head";

    const arrow = document.createElement("button");
    arrow.type = "button";
    arrow.className = "li-arrow";
    arrow.title = "展开 / 收起";
    arrow.setAttribute("aria-label", "展开");
    head.append(arrow);

    // 图标磁贴：列表项是通用形状，统一用 file——前端不许按字段名挑图标（ADR-015）
    const ico = document.createElement("span");
    ico.className = "li-ico";
    ico.innerHTML = Icons.svg("file", 16);
    head.append(ico);

    // 两行摘要：第一段当标题，其余段当描述（demo 的 .list-row 结构）。
    // 会话 52：标题行单独包一层，测试状态灯跟在名字**后面**（小一号、留间距），
    // 不再挂在「删除」最右侧——离名字近才能一眼看出「这套配置测没测过」。
    const parts = summaryParts(f, it);
    const info = document.createElement("div");
    info.className = "li-info";
    const nameRow = document.createElement("div");
    nameRow.className = "li-name-row";
    const nm = document.createElement("span");
    nm.className = "li-name";
    nm.textContent = parts[0] || "未命名";
    nameRow.append(nm);
    info.append(nameRow);
    let de = null;
    if (parts.length > 1) {
      de = document.createElement("span");
      de.className = "li-desc";
      de.textContent = parts.slice(1).join(" · ");
      info.append(de);
    }
    sums.set(it.id, { nm, de });
    head.append(info);

    // 「测试」在「删除」左侧：先用这套配置实测一句，不写入配置。
    // 状态灯现在跟在名字后面（.li-name-row，会话 52 用户改版）。
    let dot = null;
    if (f.test_action) {
      const test = document.createElement("button");
      test.type = "button";
      test.className = "btn li-test";
      test.textContent = "测试";
      test.title = "用这套配置实测一句翻译（不写入配置）";
      test.addEventListener("click", () => {
        const fresh = readItems().find((x) => x.id === it.id) || it;
        testProfile(f.test_action, fresh, (res) => {
          const status = res && res.ok !== false ? "ok" : "bad";
          setProfileStatus(f.key, fresh, status);
          const dot = dots.get(it.id);
          if (dot) markStatus(dot, status);
        }, test);
      });
      head.append(test);

      // 绿=通过 / 红=失败 / 灰=未测试。卡片里不再插结果文本框，
      // 测完只看这颗灯；尺寸/发光由 CSS 管，别在这里写死。
      dot = document.createElement("span");
      dot.className = "li-status";
      markStatus(dot, getProfileStatus(f.key, it));
      dots.set(it.id, dot);
      nameRow.append(dot);
    }

    const del = document.createElement("button");
    del.type = "button";
    del.className = "btn li-del";
    del.textContent = "删除";
    del.title = "只删这套配置，不动任何账号";
    del.addEventListener("click", () => removeItem(it.id));
    head.append(del);

    // 卡头启用开关（会话 53 用户裁定）：关掉后这套配置**整个**从「翻译模式」
    // 下拉里消失；厂商详情里的小开关继续只管「大模型」选项。老配置没有
    // enabled 字段，默认视为启用。改动走 patch → pending，保存才落盘。
    const en = itemControl({ type: "switch" }, it.enabled !== false, (v) =>
      patch(it.id, "enabled", v),
    );
    en.title = "关闭后不出现在「翻译引擎」列表";
    en.classList.add("li-enable");
    head.append(en);

    c.append(head);

    const detail = document.createElement("div");
    detail.className = "li-detail hidden";
    details.set(it.id, detail);
    fillDetail(detail, it);
    c.append(detail);

    arrow.addEventListener("click", () => {
      const open = !c.classList.contains("open");
      c.classList.toggle("open", open);
      detail.classList.toggle("hidden", !open);
    });

    return c;
  }

  function removeItem(id) {
    const next = readItems().filter((x) => x.id !== id);
    commit(f.key, next);
    // 删的正好是「当前使用」→ 顺位：厂商优先、OpenAI 其次。
    // 厂商的 active_engine 可能带 :llm 后缀（机翻/大模型），两种都要算命中。
    const prefix = f.key === "vendor_profiles" ? "vendor:" : "openai:";
    const ae = String(val("active_engine") || "");
    if (ae === prefix + id || ae === prefix + id + ":llm") {
      const vendors = f.key === "vendor_profiles" ? next : (val("vendor_profiles") || []);
      const models = f.key === "openai_profiles" ? next : (val("openai_profiles") || []);
      const pick = vendors[0]
        ? "vendor:" + vendors[0].id
        : models[0]
        ? "openai:" + models[0].id
        : "microsoft";
      commit("active_engine", pick);
    }
    renderFields();
  }

  function addItem() {
    // 后端给的 label 是「已配置的厂商 / 已配置的模型」，去掉前缀正好当弹窗标题，
    // 不必再往 schema 里加一个 add_title 字段，也不在这里硬编码字段名。
    const noun = String(f.label || "一条").replace(/^已配置的/, "") || "一条";
    openFormDialog("添加" + noun, itemFields, f.test_action, (draft, dialogStatus) => {
      const item = Object.assign({ id: newItemId() }, draft);
      // 弹窗里测过就把状态带到新卡片，否则用户会觉得「刚测通过怎么是灰的」。
      // 用含 id 的 item 重算指纹，和卡片渲染时的 stableJson 对得上。
      if (dialogStatus && dialogStatus !== "idle") {
        setProfileStatus(f.key, item, dialogStatus);
      }
      const next = readItems();
      next.push(item);
      commit(f.key, next);
      // 第一个添加的直接设为当前使用，否则用户以为没生效
      if (!String(val("active_engine") || "")) {
        const prefix = f.key === "vendor_profiles" ? "vendor:" : "openai:";
        commit("active_engine", prefix + item.id);
      }
      renderFields();
    });
  }

  function draw() {
    body.textContent = "";
    sums.clear();
    details.clear();
    const items = readItems();

    if (!items.length) {
      const empty = document.createElement("div");
      empty.className = "list-empty";
      empty.textContent = "还没有添加。点下面的「添加」新建一套。";
      body.append(empty);
    }
    for (const it of items) body.append(card(it));

    const add = document.createElement("button");
    add.type = "button";
    add.className = "btn list-add";
    add.innerHTML = Icons.svg("plus", 15) + "<span>添加</span>";
    add.addEventListener("click", addItem);
    body.append(add);
  }

  draw();
  return w;
}

// ── 表单弹窗（新建一条配置）────────────────────────────────────────────
//
// 字段来自后端 `item_fields`，只支持 text / password / select / switch。
// `show_if` 读的是**草稿对象**，切厂商时重画一次，让另一家的凭据行跟着出现 / 消失。

function openFormDialog(title, fields, testAction, onSave) {
  const draft = {};
  // 弹窗这一轮的测试结论；改了任何字段就作废，免得拿旧配置的「通过」
  // 去标一套已经改过的配置。
  let dialogStatus = "idle";
  for (const fld of fields) {
    // select 的草稿值必须等于界面上默认选中的那一项：浏览器会自动选中第一个
    // option，但草稿若留空，项内 show_if 就认为「没选厂商」，两家的凭据行
    // 全被藏起来，连 APPID 都没地方填。会话 31 实机发现的坑。
    draft[fld.key] = fld.type === "switch"
      ? false
      : fld.type === "select"
        ? String((fld.options && fld.options[0] && fld.options[0][0]) || "")
        : "";
  }

  el("fm-title").textContent = title;
  const body = el("fm-body");

  // 弹窗里的「测试」测的是**当前草稿**（还没保存、甚至还没进列表）。
  // 只有后端声明了 test_action 的列表才有这个按钮。close() 只把弹窗藏起来，
  // body 不清，所以每次打开都要手动收掉上一轮留下的结论框。
  // 结论框只写一句话，不摊开译文 / 语向 / 耗时（会话 33）。
  const clearResult = () => {
    const old = body.querySelector(".result");
    if (old) old.remove();
  };
  const testBtn = el("fmTest");
  testBtn.classList.toggle("hidden", !testAction);
  clearResult();
  testBtn.onclick = testAction
    ? () => testProfile(testAction, JSON.parse(JSON.stringify(draft)), (res) => {
        dialogStatus = res && res.ok !== false ? "ok" : "bad";
        renderSimpleResult(body, dialogStatus);
      }, testBtn)
    : null;

  // 项内联动只看草稿自己，复用卡片详情那一套判断
  const shown = (fld) => itemShowIf(fld, draft);

  function draw() {
    body.textContent = "";
    for (const fld of fields) {
      if (!shown(fld)) continue;
      const row = document.createElement("div");
      row.className = "fm-row" + (fld.type === "switch" ? " fm-switch" : "");

      const lab = document.createElement("span");
      lab.className = "fm-label";
      lab.textContent = fld.label || fld.key;
      row.append(lab);

      const ctrl = itemControl(fld, draft[fld.key], (v) => {
        draft[fld.key] = v;
        // 草稿变了，上一轮的测试结论作废
        dialogStatus = "idle";
        clearResult();
        // 下拉切换会改变「哪些字段该显示」，必须重画
        if (fld.type === "select") draw();
      });
      row.append(ctrl);
      body.append(row);
    }
  }

  function close() {
    el("form-modal").classList.add("hidden");
  }

  el("fmSave").onclick = () => {
    // 名称留空给个占位，别让卡片出现空标题。认 name / vendor / model
    // 三个通用键（两张列表都是这个形状），不是某个工具专属分支。
    // 厂商没填名字就叫「百度」/「有道」，模型没填就用模型名。
    if (!String(draft.name || "").trim()) {
      const vf = fields.find((x) => x.key === "vendor");
      const hit = vf && (vf.options || []).find(
        (o) => String(o[0]) === String(draft.vendor || "")
      );
      draft.name = (hit && String(hit[1]))
        || String(draft.model || "").trim()
        || "未命名";
    }
    close();
    onSave(draft, dialogStatus);
  };
  el("fmCancel").onclick = close;

  draw();
  el("form-modal").classList.remove("hidden");
  const first = body.querySelector("input[type=text], select");
  if (first) first.focus();
}


function fieldInput(f, inputType) {
  const [w, ctrl] = fieldRow(f);
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

  ctrl.append(inp);

  // 预设按钮（目前只有「默认 User-Agent」用）：点一下把值填进同一个输入框并保存。
  // 走和手填完全一样的 commit 路径，所以后端校验、回读、状态行刷新都一致。
  if (Array.isArray(f.presets) && f.presets.length) {
    const row = document.createElement("div");
    row.className = "presets";
    for (const [name, value] of f.presets) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "preset-btn";
      b.textContent = name;
      b.title = value;
      b.addEventListener("click", () => {
        inp.value = value;
        commit(f.key, value);
      });
      row.append(b);
    }
    ctrl.append(row);
  }
  return w;
}

/// 命名模板：文本框 + 格式预设 + 预览（会话 81）。
///
/// 预设按钮只替换 `$...$` 里的格式，保留用户自己写的前缀 / 后缀：
/// `Vexto_$yyyyMMdd_HHmmss$.png` 点 UUID → `Vexto_$uuid$.png`。
/// 预览走 `preview_command`（后端渲染真实时间 / UUID），结果显示在按钮旁边。
function fieldTemplate(f) {
  const [w, ctrl] = fieldRow(f);
  const inp = document.createElement("input");
  inp.type = "text";
  inp.value = val(f.key) == null ? "" : String(val(f.key));
  if (f.placeholder) inp.placeholder = f.placeholder;
  inp.spellcheck = false;
  inp.autocomplete = "off";
  ctrl.append(inp);

  const out = document.createElement("div");
  out.className = "tpl-preview hidden";

  const applyToken = (token) => {
    const cur = inp.value;
    const m = /\$[^$]*\$/.exec(cur);
    let next;
    if (m) {
      next = cur.slice(0, m.index) + "$" + token + "$" + cur.slice(m.index + m[0].length);
    } else {
      const ext = /\.png$/i.exec(cur);
      next = ext
        ? cur.slice(0, ext.index) + "$" + token + "$" + cur.slice(ext.index)
        : cur + "$" + token + "$";
    }
    inp.value = next;
    out.classList.add("hidden");
    commit(f.key, next);
  };

  if (Array.isArray(f.presets) && f.presets.length) {
    const row = document.createElement("div");
    row.className = "presets";
    for (const [name, token] of f.presets) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "preset-btn";
      b.textContent = name;
      b.title = "$" + token + "$";
      b.addEventListener("click", () => applyToken(token));
      row.append(b);
    }
    ctrl.append(row);
  }

  if (f.preview_command) {
    const row = document.createElement("div");
    row.className = "tpl-preview-row";
    const b = document.createElement("button");
    b.type = "button";
    b.className = "btn";
    b.textContent = "预览";
    const run = async () => {
      b.disabled = true;
      try {
        const name = await invoke(f.preview_command, { rule: inp.value });
        out.textContent = "示例：" + name;
        out.classList.remove("bad");
      } catch (e) {
        out.textContent = errText(e);
        out.classList.add("bad");
      } finally {
        out.classList.remove("hidden");
        b.disabled = false;
      }
    };
    b.addEventListener("click", run);
    row.append(b, out);
    ctrl.append(row);
  }

  inp.addEventListener("input", () => out.classList.add("hidden"));
  inp.addEventListener("blur", () => commit(f.key, inp.value.trim()));
  inp.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      inp.blur();
    }
  });
  return w;
}

function fieldNumber(f) {  const [w, ctrl] = fieldRow(f);

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

  ctrl.append(row);
  return w;
}

function fieldSwitch(f) {
  const [w, ctrl] = fieldRow(f, "switch");
  const inp = document.createElement("input");
  inp.type = "checkbox";
  inp.checked = !!val(f.key);
  const track = document.createElement("span");
  track.className = "track";
  const thumb = document.createElement("span");
  thumb.className = "thumb";
  // v2：勾选框换成 pill 开关外观（demo 口径）。仍是 <input type=checkbox>
  // + change 事件，保存路径零改动。控件类名用 switch-pill——包装层也带
  // "switch" 这个 extraClass，裸 .switch 选择器会命中它（实测踩过）。
  const lab = document.createElement("label");
  lab.className = "switch-pill";
  lab.append(inp, track, thumb);
  // 点标题也能切换（保留原交互；meta 和开关是两个节点，不会重复触发）
  w.querySelector(".meta").addEventListener("click", () => {
    inp.checked = !inp.checked;
    inp.dispatchEvent(new Event("change"));
  });
  inp.addEventListener("change", () => commit(f.key, inp.checked));
  ctrl.append(lab);
  return w;
}

// 「翻译模式」的候选项：从当前配置里的厂商实例 + OpenAI 模型算出来。
// 值就是 `active_engine` 的写法（`vendor:<id>` / `vendor:<id>:llm` / `openai:<id>`）。
// 开了 use_llm 的厂商出两项：机翻 + 大模型（会话 52，后端 engine_choices 同口径）；
// 卡头开关关掉的实例（enabled=false，会话 53）整条不出现。
function engineChoices() {
  const listOf = (k) => {
    const v = val(k);
    return Array.isArray(v) ? v : [];
  };
  const out = [];
  // 内置免配置引擎永远排在第一个；它不是「厂商管理」里的实例（ADR-058）。
  out.push(["microsoft", "微软翻译（免配置）"]);
  for (const p of listOf("vendor_profiles")) {
    if (p.enabled === false) continue;
    const name = String(p.name || "").trim() || "未命名";
    out.push(["vendor:" + p.id, p.use_llm ? name + " · 机翻" : name]);
    if (p.use_llm) {
      out.push(["vendor:" + p.id + ":llm", name + " · 大模型"]);
    }
  }
  for (const p of listOf("openai_profiles")) {
    if (p.enabled === false) continue;
    const name = String(p.name || "").trim() || "未命名";
    out.push(["openai:" + p.id, "OpenAI 兼容 · " + name]);
  }
  return out;
}

/// 填充一个动态下拉。候选为空时给一条禁用占位，别让用户对着空框发愣。
function fillEngineSelect(sel, opts, cur) {
  sel.textContent = "";
  sel.disabled = !opts.length;
  if (!opts.length) {
    const o = document.createElement("option");
    o.value = "";
    o.textContent = "（还没有配置，先在下面添加）";
    sel.append(o);
    return;
  }
  // options 是 [[value, label], ...] 的二元组数组
  for (const [v, txt] of opts) {
    const o = document.createElement("option");
    o.value = v;
    o.textContent = txt;
    if (String(cur) === String(v)) o.selected = true;
    sel.append(o);
  }
}

/// 候选变了就**原地**重建 options，不换 select 节点（焦点与选中态都不丢）。
/// 用候选签名去重：逐字输入名称时不会每次都动 DOM。
/// 由 `commit()` 统一调用，所以列表里的改名 / 切开关 / 增删全都覆盖得到。
function refreshEngineSelects() {
  if (!engineSelectNodes.length) return;
  const opts = engineChoices();
  const sig = JSON.stringify(opts);
  for (const sel of engineSelectNodes) {
    if (!sel.isConnected || sel.dataset.optSig === sig) continue;
    sel.dataset.optSig = sig;
    // 保留用户刚选的值；旧值已不在候选里时浏览器会停在第一项，
    // 写回由 `removeItem()` 的顺位与后端 `migrate_vendors()` 兜底。
    fillEngineSelect(sel, opts, val(sel.dataset.key || ""));
  }
}

function fieldSelect(f) {
  const [w, ctrl] = fieldRow(f);
  const sel = document.createElement("select");

  if (f.dynamic) {
    // dynamic 的选项来自同页配置，`options` 是后端给的空表，作废
    sel.dataset.key = f.key;
    engineSelectNodes.push(sel);
    const opts = engineChoices();
    fillEngineSelect(sel, opts, val(f.key));
    sel.dataset.optSig = JSON.stringify(opts);
  } else {
    const cur = val(f.key);
    let matched = false;
    for (const [v, txt] of f.options || []) {
      const o = document.createElement("option");
      o.value = v;
      o.textContent = txt;
      if (String(cur) === String(v)) {
        o.selected = true;
        matched = true;
      }
      sel.append(o);
    }
    // 老配置里可能存着不在预设表里的值（例如旧版手填的 DNS）。原样补一项，
    // 不要因为下拉没有这个值就把界面显示成第一项、让用户以为配置被改了。
    if (cur != null && String(cur) !== "" && !matched) {
      const o = document.createElement("option");
      o.value = String(cur);
      o.textContent = String(cur);
      o.selected = true;
      sel.append(o);
    }
  }
  sel.addEventListener("change", () => commit(f.key, sel.value, { autoApply: !!f.auto_apply }));
  ctrl.append(sel);
  return w;
}

/// 主色调（外观功能）：一排预设色块 + 「自定义」原生取色。
/// 色块颜色是**数据**（后端色板给的用户可选值），不是 UI 装饰色，
/// 所以这里用内联 style 上色不违反「裸色值不出现在 CSS」的口径。
/// 点已选中的色块 = 恢复默认（存空串）；自定义取色随时可换任意色。
function fieldColor(f) {
  const [w, ctrl] = fieldRow(f);
  const row = document.createElement("div");
  row.className = "swatches";

  const cur = String(val(f.key) || "");
  const active = cur || f.presets?.[0]?.[0] || "";

  for (const [value, name] of f.presets || []) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "swatch" + (value === active ? " on" : "");
    b.style.background = value;
    b.title = name;
    b.setAttribute("aria-label", name);
    b.addEventListener("click", () => {
      // 再点一下当前色 = 清空回默认；否则换色
      commit(f.key, value === active ? "" : value, { autoApply: !!f.auto_apply });
    });
    row.append(b);
  }

  // 自定义：原生 <input type="color">，包一层当「色块」看。
  // 当前值不在预设里时，让它显示当前自定义色。
  const custom = document.createElement("label");
  custom.className = "swatch custom";
  custom.title = "自定义";
  const pick = document.createElement("input");
  pick.type = "color";
  pick.value = /^#[0-9a-fA-F]{6}$/.test(cur) ? cur : "#7c5cff";
  pick.addEventListener("input", () => commit(f.key, pick.value, { autoApply: !!f.auto_apply }));
  custom.append(pick);
  row.append(custom);

  ctrl.append(row);
  return w;
}

// ── 热键捕获状态机（规格见 docs/stages/S2-配置与设置中心.md §2.6.3 / ADR-018）──
const MODS = ["Ctrl", "Shift", "Alt"];              // 白名单，顺序即归一化顺序
const MOD_KEYS = { Control: "Ctrl", Shift: "Shift", Alt: "Alt" };
const MAIN_RE = /^(Key[A-Z]|Digit[0-9]|F([1-9]|1[0-2])|PrintScreen)$/;   // 主键白名单（S7 加 PrintScreen）

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
             : e.code;                        // F1..F12 / PrintScreen 原样
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
  const [w, ctrl] = fieldRow(f);
  const inp = document.createElement("input");
  inp.type = "text";
  inp.className = "capture";
  inp.readOnly = true;
  inp.spellcheck = false;
  inp.value = val(f.key) == null ? "" : String(val(f.key));
  inp.placeholder = "点击设置";
  inp.addEventListener("click", () => beginCapture(inp, f.key));
  inp.addEventListener("focus", () => beginCapture(inp, f.key));
  ctrl.append(inp);
  return w;
}

function fieldButton(f) {
  const w = wrapField(f, "action");
  const b = document.createElement("button");
  b.type = "button";
  b.className = "btn";
  b.textContent = f.label || f.key;
  b.addEventListener("click", () => runAction(f.key, b));
  const row = document.createElement("div");
  row.className = "btn-row";
  row.append(b);
  if (f.help) row.append(helpIcon(f.help));
  w.append(row);
  return w;
}

/// 一排并排按钮（打开配置 / 重启服务这类成对动作），靠右排列。
function fieldButtonRow(f) {
  const w = wrapField(f, "action");
  const row = document.createElement("div");
  row.className = "btn-row";
  for (const [key, label] of f.actions || []) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "btn";
    b.textContent = label;
    b.addEventListener("click", () => runAction(key, b));
    row.append(b);
  }
  w.append(row);
  return w;
}

// 动作型字段：key 就是后端的命令名，前端不认识具体业务。
// 反馈统一走右下角 toast（自动消失），不在表单里堆常驻结果框。
async function runAction(key, btn) {
  btn.disabled = true;
  try {
    const res = await invoke(key);
    showBanner("");
    const bad = res && res.ok === false;
    showToast((res && res.message) || (bad ? "操作失败" : "完成"), bad ? "bad" : "");
    // 动作可能改了引擎状态（「重启下载引擎」就会），状态行必须跟着刷新，
    // 否则用户看到「aria2c 未启动」还以为按钮没生效。
    await refreshAfterSave();
  } catch (e) {
    showToast(errText(e), "bad");
  } finally {
    btn.disabled = false;
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
    n.classList.toggle("hidden", !showIfMatch(cond));
  }
}

/// 一条联动条件是否成立。
///
/// `equals` 是数组（命中任一即算满足），`also` 是「同时满足」的第二层条件
/// ——翻译引擎拆成「模式 + 厂商」后，凭据字段必须同时看两个字段。
/// 旧后端给的是字符串 `equals`，这里兼容一下，混版本时不至于整页字段乱显隐。
function showIfMatch(cond) {
  if (!cond || !cond.key) return true;
  const list = Array.isArray(cond.equals) ? cond.equals : [cond.equals];
  const cur = String(val(cond.key));
  if (!list.some((e) => String(e) === cur)) return false;
  return cond.also ? showIfMatch(cond.also) : true;
}

// ── 保存 / 放弃 ─────────────────────────────────────────────────────────
//
// 改动只进 `state.pending`，不再「改一下就存」。这样用户有一整个「这批改动」
// 的概念，可以反悔；代价是状态摘要要等保存后才刷新。

function renderDirty() {
  const n = pendingCount();
  el("dirty-acts").classList.toggle("hidden", n === 0);
  if (n > 0) {
    setStatus(`有 ${n} 项未保存`, "dirty");
  } else if (el("status").classList.contains("dirty")) {
    setStatus("");
  }
}

/// 值相等判断：list 字段是对象数组，而保存前后对象键顺序可能不同
/// （前端草稿按表单字段顺序，后端按 Rust 结构顺序），所以不能直接 JSON.stringify 比。
function stableJson(v) {
  if (v === null || typeof v !== "object") return JSON.stringify(v);
  if (Array.isArray(v)) return "[" + v.map(stableJson).join(",") + "]";
  return "{" + Object.keys(v).sort()
    .map((k) => JSON.stringify(k) + ":" + stableJson(v[k])).join(",") + "}";
}

/// 一个字段被改动：默认只记待保存、**不打请求**；带 `opts.autoApply`（字段
/// schema 里标了 `auto_apply`，如外观两项）则**改完立即落盘生效**，完全不进
/// 「未保存」列表——所见即所得，不再弹保存/放弃（会话 57 用户裁定）。
// autoApply 的防抖计时器（按 key 分桶；见 commit 内注释）
const autoApplyTimers = {};
///
/// 会话 32：只在值真的变了才进 pending。以前 blur 一下、下拉重选同一个值
/// 都会写一条改动，用户什么都没改也看到「有 N 项未保存」；改回原值也要
/// 把这条 pending 撤掉，而不是留着一个和磁盘一致的「假改动」。
function commit(key, value, opts = {}) {
  if (opts.autoApply) {
    if (stableJson(state.values[key]) !== stableJson(value)) {
      state.values[key] = value; // val() 立刻读到新值（联动 / 指纹一致）
      // 取色器拖动会连续触发 input：250ms 内的改动合并成一次落盘，
      // 预览（vexto:committed → theme.js）仍然是每个事件即时生效。
      clearTimeout(autoApplyTimers[key]);
      autoApplyTimers[key] = setTimeout(() => {
        delete autoApplyTimers[key];
        invoke("save_tool_config", {
          toolId: state.toolId,
          values: { [key]: value },
        })
          .then((res) => {
            if (!res || !res.ok) showBanner((res && res.message) || "外观保存失败");
          })
          .catch((e) => showBanner("外观保存失败：" + errText(e)));
      }, 250);
    }
    applyShowIf();
    renderDirty();
    refreshEngineSelects();
    document.dispatchEvent(
      new CustomEvent("vexto:committed", { detail: { key, value } }),
    );
    return;
  }
  const original = state.values[key];
  if (stableJson(original) === stableJson(value)) {
    delete state.pending[key];
  } else {
    state.pending[key] = value;
  }
  applyShowIf();          // 立刻重算联动，不等保存
  renderDirty();
  // 列表里改名 / 切大模型开关后，上面的动态下拉文字要跟着变。
  // 这里统一刷，避免在每个 list 分支里各写一遍；函数内部按签名去重，
  // 所以逐字输入不会反复动 DOM。
  refreshEngineSelects();
  // 外观字段（主题 / 主色调）改动要能**未保存就预览**：theme.js 在文档上
  // 收这个事件、按 key 现算现应用。这里只广播「有改动」，不认识任何 key。
  document.dispatchEvent(
    new CustomEvent("vexto:committed", { detail: { key, value } }),
  );
}

/// 把 pending 里的所有改动一次交出去。返回是否成功。
async function saveAll() {
  if (pendingCount() === 0) return true;
  setStatus("保存中…");
  try {
    const res = await invoke("save_tool_config", {
      toolId: state.toolId,
      values: state.pending,
    });
    if (!res || !res.ok) {
      setStatus((res && res.message) || "保存失败", true);
      showBanner((res && res.message) || "保存失败");
      return false;
    }
    state.pending = {};
    // 后端会改写部分值（清空目录回写默认路径），所以保存后重读一次。
    await reloadValues();
    renderDirty();
    setStatus(res.message || "已保存");
    if (Array.isArray(res.warnings) && res.warnings.length) {
      showBanner(res.warnings.join("\n"));
    } else {
      // 没有新警告就收掉旧横幅；refreshAfterSave 里的热键检查会把仍存在的
      // 问题（如热键仍被占用）重新挂上来。
      showBanner("");
    }
    // 热键归一化、状态摘要都变了，回读一次
    await refreshAfterSave();
    return true;
  } catch (e) {
    setStatus("保存失败：" + errText(e), true);
    showBanner("保存失败：" + errText(e));
    return false;
  }
}

/// 丢掉所有未保存改动，回到后端真实值。
async function discardAll() {
  state.pending = {};
  await reloadValues();
  renderDirty();
  setStatus("已放弃修改");
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

// ── 未保存改动的确认框 ──────────────────────────────────────────────────
//
// 只在「有 pending 又要离开」时出现：切工具、关窗口。三种出口都给了：
// 保存 / 放弃 / 取消，用户不需要猜「点了会怎样」。

/// 离开动作：`{kind:"close"}` 或 `{kind:"switch", toolId}`。
let dirtyIntent = null;

function showDirtyModal(intent) {
  dirtyIntent = intent;
  const n = pendingCount();
  const what = el("tool-name").textContent || "当前工具";
  el("modal-title").textContent = "有未保存的修改";
  el("modal-text").textContent = intent.kind === "close"
    ? `「${what}」有 ${n} 项改动还没保存，直接关闭会丢掉它们。`
    : `「${what}」有 ${n} 项改动还没保存，切换工具会丢掉它们。`;
  el("dirty-modal").classList.remove("hidden");
  el("mSave").focus();
}

function hideDirtyModal() {
  el("dirty-modal").classList.add("hidden");
  dirtyIntent = null;
}

/// 保存/放弃之后再执行真正的离开动作。
function runIntent(intent) {
  if (!intent) return;
  if (intent.kind === "close") {
    invoke("window_action", { action: "close" }).catch(() => {});
  } else if (intent.kind === "switch") {
    selectTool(intent.toolId, { skipGuard: true });
  }
}

// ── 启动 ─────────────────────────────────────────────────────────────────

async function boot() {
  Theme.apply(); // 外观：主题三态 + 主色调（本页改动经 commit 事件即时预览）
  await reloadTools();

  // 托盘点了某个工具、或从别处跳过来：定位到它
  let wanted = null;
  try {
    wanted = await invoke("take_pending_tool");
  } catch {
    /* 没有就算了 */
  }

  const first = wanted && state.tools.some((t) => t.id === wanted)
    ? wanted
    : (state.tools[0] && state.tools[0].id);
  await selectTool(first);

  // ── 手动保存的两个按钮 ────────────────────────────────────────────
  el("btnSave").addEventListener("click", () => saveAll());
  el("btnDiscard").addEventListener("click", () => discardAll());

  // ── 未保存改动的确认框 ────────────────────────────────────────────
  el("mCancel").addEventListener("click", hideDirtyModal);
  el("mDiscard").addEventListener("click", async () => {
    const intent = dirtyIntent;
    hideDirtyModal();
    state.pending = {};
    renderDirty();
    setStatus("已放弃修改");
    if (intent) runIntent(intent);
    else await discardAll();
  });
  el("mSave").addEventListener("click", async () => {
    const intent = dirtyIntent;
    const btn = el("mSave");
    btn.disabled = true;
    const ok = await saveAll();
    btn.disabled = false;
    if (!ok) return;          // 保存失败就停在弹窗里，让用户看到原因
    hideDirtyModal();
    runIntent(intent);
  });

  // 关窗口前问一句。`titlebar.js` 的关闭按钮会先调它，返回 false 就不关。
  window.__beforeClose = async () => {
    if (pendingCount() === 0) return true;
    showDirtyModal({ kind: "close" });
    return false;
  };

  renderDirty();

  // 设置窗口已经开着时再跳转：走事件而不是重载页面
  await listen("settings-goto", async (ev) => {
    const id = ev && ev.payload;
    if (id && state.tools.some((t) => t.id === id)) {
      await selectTool(id, { keepBanner: true });
    }
  });

  // 标题栏的明暗切换会直接写配置并广播：当前停在通用页且没有未保存改动时，
  // 把下拉同步到新值，避免界面显示和磁盘不一致。
  await listen("appearance:changed", (ev) => {
    const p = (ev && ev.payload) || {};
    if (typeof state.values.appearance_theme === "string") {
      state.values.appearance_theme = p.theme;
    }
    if (typeof state.values.accent_color === "string") {
      state.values.accent_color = p.accent;
    }
    if (state.toolId === "general" && pendingCount() === 0) renderFields();
  });

  // 首屏（工具列表 + 字段值）都填好了，才让后端把窗口显示出来
  window.TitleBar?.reveal?.();
}

window.addEventListener("DOMContentLoaded", boot);
