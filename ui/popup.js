/* 翻译弹窗前端。
 *
 * 两种模式共用一个窗口，靠 payload 的 `mode` 区分：
 *   - `select`（划词翻译）：窗口不聚焦，保住用户选中的文字；源文本只读展示
 *   - `input`（输入翻译）：窗口可聚焦，源文本框是第一焦点，回车即翻译
 * 窗口的 focusable 由后端按 mode 切换（见 src-tauri/src/popup.rs）。
 *
 * 渲染必须**先复位再分支**：错误重试时如果不清上一轮状态，
 * 会出现"错误卡片上还挂着上次的译文"（S1b 踩过的坑）。 */

const T = window.__TAURI__;
const invoke = T.core.invoke;

/* 阴影边距，必须与 popup.css 的 #card margin 和 popup.rs 的 CARD_PAD_* 一致 */
const PAD_TOP = 8;
const PAD_X = 12;
const PAD_BOTTOM = 16;

const el = (id) => document.getElementById(id);

// 图标全部来自 icons.js（24 viewBox / stroke 1.6 / currentColor）。
// 「复制译文」按钮里的文字要保持，所以图标插在文字前面而不是整体替换。
(function mountIcons() {
  const I = window.Icons;
  I.fill(el("pin"), "pin", 14);
  I.fill(el("close"), "close", 14);
  I.fill(el("copy-src"), "copy", 14);
  I.fill(el("open-settings"), "gear", 16);
  el("copy-dst").insertAdjacentHTML("afterbegin", I.svg("copy", 14));
})();

let lastPayload = null;
let hideTimer = null;
let hovering = false;
/** 当前模式：select | input */
let mode = "select";
/** 图钉是否钉住。钉住时：不自动关闭、不响应点击外部（后端也在管） */
let pinned = false;
/** 引擎列表与当前引擎，由后端 payload 给 */
let engines = [];
let currentEngine = "";

function timeoutMs() {
  const t = lastPayload && lastPayload.timeout;
  return Number.isFinite(t) && t > 0 ? t : 15000;
}

function closePopup() {
  clearTimeout(hideTimer);
  invoke("close_popup").catch(() => {});
}

/** 排定自动关闭。钉住时永不排定。 */
function scheduleHide(ms) {
  clearTimeout(hideTimer);
  if (pinned) return; // 钉住了就不许自己消失
  hideTimer = setTimeout(closePopup, ms || timeoutMs());
}

/** 量卡片高度告诉后端（ADR-011：原生窗口尺寸只能后端设）。 */
function reportHeight() {
  const card = el("card");
  const r = card.getBoundingClientRect();
  const style = getComputedStyle(card);
  const mt = parseFloat(style.marginTop) || 0;
  const mb = parseFloat(style.marginBottom) || 0;
  const ml = parseFloat(style.marginLeft) || 0;
  const mr = parseFloat(style.marginRight) || 0;
  const h = Math.ceil(r.height + mt + mb);
  const w = Math.ceil(r.width + ml + mr);
  invoke("resize_popup", { height: Math.max(60, h), width: Math.max(120, w) }).catch(
    () => {}
  );
}

/** 源文本框按内容撑高（最多 max-height，超出滚动）。 */
function autoGrowSrc() {
  const t = el("src");
  t.style.height = "auto";
  t.style.height = Math.min(t.scrollHeight, 96) + "px";
}

function setPinned(on) {
  pinned = on;
  const b = el("pin");
  b.setAttribute("aria-pressed", String(on));
  b.title = on ? "已钉住（不自动关闭，点此取消）" : "钉住（不自动关闭）";
  if (on) {
    clearTimeout(hideTimer);
  } else if (lastPayload && lastPayload.state === "done") {
    // 取消钉住后重新开始倒计时，否则它会一直挂着
    scheduleHide();
  }
  invoke("set_pinned", { pinned: on }).catch(() => {});
}

function renderEngines() {
  const menu = el("engine-menu");
  menu.textContent = "";
  for (const e of engines) {
    const b = document.createElement("button");
    b.className = "engine-item" + (e.id === currentEngine ? " on" : "");
    b.textContent = e.name;
    b.dataset.engineId = e.id;
    b.addEventListener("click", () => {
      hideEngineMenu();
      if (e.id === currentEngine) return;
      currentEngine = e.id;
      el("engine-name").textContent = e.name;
      // 切换引擎写进配置，然后**用当前源文本重翻一次**。
      // 用户点引擎的意图就是"我要用这个引擎翻这段"，不重翻等于什么都没发生。
      invoke("switch_engine", { engineId: e.id })
        .then((res) => {
          if (res && res.ok === false) return;
          doTranslate();
        })
        .catch(() => {});
    });
    menu.appendChild(b);
  }
}

function showEngineMenu() {
  if (!engines.length) return;
  renderEngines();
  el("engine-menu").classList.remove("hidden");
  // 菜单在文档流里，卡片自己就变高了，这里只是让窗口跟着长
  requestAnimationFrame(() => requestAnimationFrame(reportHeight));
}

function hideEngineMenu() {
  el("engine-menu").classList.add("hidden");
  // 收起后窗口要缩回去，不然卡片下方留一片空白
  requestAnimationFrame(() => requestAnimationFrame(reportHeight));
}

/** 复位到「新一轮」的样子。 */
function resetCard() {
  hideEngineMenu();
  el("loading").classList.add("hidden");
  el("dst").classList.add("hidden");
  el("dst").textContent = "";
  el("error").classList.add("hidden");
  el("error").textContent = "";
  el("actions").classList.remove("on");
  el("copy-src").classList.add("hidden");
  el("src").readOnly = false;
}

function render(p) {
  lastPayload = p;
  mode = p.mode || "select";
  pinned = !!p.pinned;
  engines = Array.isArray(p.engines) ? p.engines : [];
  currentEngine = p.engine || "";

  el("pin").setAttribute("aria-pressed", String(pinned));
  el("pin").title = pinned ? "已钉住（不自动关闭，点此取消）" : "钉住（不自动关闭）";

  // 先复位再分支：不复位的话「error → 重译成功」会同时显示错误框和译文
  // （S1b 踩过的坑，注释见文件头）。
  // 复位也会重置 copy-src 的可见性，所以它必须在下面那行之前。
  resetCard();

  // 「翻译」按钮两种模式都要有：划词弹窗里用户改了源文本或切了引擎之后，
  // 需要一个明确的"翻这段"的入口（用户 m03664 报的第 1 个 bug）。
  // 之前只在输入模式显示，划词模式改完字只能靠回车，而划词窗口默认不聚焦，
  // 回车根本到不了前端。
  el("copy-src").classList.toggle("hidden", mode === "input");

  // 源文本：**输入模式下永远不覆盖**。用户正在打的东西是权威，
  // 后端在 loading 时发的是空串，一旦写回去用户就发现自己打的字没了。
  // 划词模式下源文本来自后端取词，才需要回填。
  const srcEl = el("src");
  if (mode !== "input") {
    if (typeof p.src === "string" && srcEl.value !== p.src) {
      srcEl.value = p.src;
    }
  } else if (p.fresh) {
    // 重新按了一次输入翻译热键 = 要一个干净的框
    srcEl.value = "";
  } else if (p.state === "done" && typeof p.src === "string" && p.src) {
    // done 时后端带回正规化后的源文本，此时用户已提交，可以回填
    srcEl.value = p.src;
  }
  autoGrowSrc();

  el("lang-pill").textContent = (p.dir && p.dir.from) || p.from || "自动";
  el("engine-name").textContent = p.engineName || "未配置引擎";

  if (p.state === "loading") {
    el("loading").classList.remove("hidden");
    el("actions").classList.remove("on");
    scheduleHide(Math.max(60000, timeoutMs()));
  } else if (p.state === "done") {
    el("dst").textContent = p.dst || "";
    el("dst").classList.remove("hidden");
    el("actions").classList.add("on");
    scheduleHide();
  } else if (p.state === "error") {
    el("error").textContent = p.message || "翻译失败";
    el("error").classList.remove("hidden");
    el("actions").classList.add("on");
    // 错误卡片留久一点，用户要读原因
    scheduleHide(Math.max(30000, timeoutMs() * 2));
  }

  requestAnimationFrame(() => requestAnimationFrame(reportHeight));
}

/* ---------- 事件绑定 ---------- */

// 拖动：在标题栏按下时交给系统移动窗口。
// 做法是 invoke start_dragging，底层 PostMessage(WM_NCLBUTTONDOWN, HTCAPTION)。
// 只在标题栏生效 —— 源文本框里按下必须能选中文字，不能变成拖窗。
el("titlebar").addEventListener("mousedown", (e) => {
  if (e.button !== 0) return;
  if (e.target.closest("#pin, #close")) return;
  invoke("start_dragging").catch(() => {});
});

el("pin").addEventListener("click", () => setPinned(!pinned));
el("close").addEventListener("click", closePopup);

el("engine-row").addEventListener("click", (e) => {
  e.stopPropagation();
  const menu = el("engine-menu");
  if (menu.classList.contains("hidden")) showEngineMenu();
  else hideEngineMenu();
});

// 点别处收起引擎菜单
document.addEventListener("click", (e) => {
  if (!e.target.closest("#engine-row, #engine-menu")) hideEngineMenu();
});

el("src").addEventListener("input", autoGrowSrc);

// 门 3 选项 A（用户 m03141）：划词模式下窗口默认不抢焦点，保住用户选中的文字。
// 但源文本是可编辑的 —— 用户点进来想改字，那一刻必须能收键盘。
// 所以焦点是「用户点了才给」，不是「弹窗一出现就给」。
//
// 只做一次：第一次点进文本框就切到可聚焦，之后一直是可聚焦，
// 反复 invoke 没有意义，还会在主线程上白白多跑几次 set_focusable。
let typingMode = false;
el("src").addEventListener("mousedown", () => {
  if (mode === "input" || typingMode) return;
  typingMode = true;
  invoke("focus_for_typing").catch(() => {});
});

// 输入模式下窗口一出现就该能打字，不用用户再点一次
function enterInputMode() {
  typingMode = true;
  const t = el("src");
  t.focus();
  const v = t.value;
  t.setSelectionRange(v.length, v.length);
}

el("src").addEventListener("keydown", (e) => {
  if (e.key !== "Enter") return;
  // Shift+Enter 换行，单回车翻译
  if (e.shiftKey) return;
  e.preventDefault();
  doTranslate();
});

el("translate-btn").addEventListener("click", doTranslate);

function doTranslate() {
  const text = el("src").value.trim();
  if (!text) return;
  clearTimeout(hideTimer);
  el("translate-btn").disabled = true;
  invoke("translate_text", { text, mode })
    .catch((e) => {
      // 命令本身报错（不是翻译失败）也要让用户看到
      lastPayload = { state: "error", message: String(e), mode };
      render(lastPayload);
    })
    .finally(() => {
      el("translate-btn").disabled = false;
    });
}

async function writeClipboard(text) {
  try {
    await navigator.clipboard.writeText(text);
  } catch (e) {
    // 不可聚焦的窗口里 clipboard API 可能被拒，交给 Rust 侧兜底
    invoke("copy_text", { text }).catch(() => {});
  }
}

/// 只改传进来的文字节点，不动按钮里的图标。
async function copyText(text, labelEl, doneLabel, idleLabel) {
  if (!text) return;
  await writeClipboard(text);
  labelEl.textContent = doneLabel;
  setTimeout(() => (labelEl.textContent = idleLabel), 1200);
}

// 复制原文是纯图标按钮：复制成功后把图标换成勾，1.2 秒后换回来
el("copy-src").addEventListener("click", async () => {
  const text = el("src").value;
  if (!text) return;
  await writeClipboard(text);
  const btn = el("copy-src");
  window.Icons.fill(btn, "check", 14);
  setTimeout(() => window.Icons.fill(btn, "copy", 14), 1200);
});

el("copy-dst").addEventListener("click", () => {
  const t = lastPayload && lastPayload.dst;
  if (!t) return;
  const span = el("copy-dst").querySelector("span");
  copyText(t, span, "已复制", "复制译文");
});

el("open-settings").addEventListener("click", () => {
  invoke("open_settings_from_popup").catch(() => {});
});

// 鼠标悬停时不自动关闭
el("card").addEventListener("mouseenter", () => {
  hovering = true;
  clearTimeout(hideTimer);
});
el("card").addEventListener("mouseleave", () => {
  hovering = false;
  if (lastPayload && lastPayload.state === "done") scheduleHide();
});

// 后端要求聚焦时（输入模式切过来），把焦点放到源文本框
T.event.listen("popup-focus", () => {
  el("src").focus();
  const v = el("src").value;
  el("src").setSelectionRange(v.length, v.length);
});

const listening = T.event.listen("popup-render", (e) => {
  render(e.payload);
  if ((e.payload && e.payload.mode) === "input") enterInputMode();
});

window.addEventListener("DOMContentLoaded", async () => {
  Theme.apply(); // 外观：主题三态 + 主色调
  try {
    await listening;
  } catch (e) {
    // 注册失败也继续走，至少能显示错误卡片
    console.error("popup-render 监听注册失败", e);
  }
  invoke("popup_ready").catch(() => {});

  // 输入模式唤出时让用户直接能打字
  if (mode === "input") enterInputMode();
});
