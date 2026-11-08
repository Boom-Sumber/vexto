// 弹窗渲染逻辑。通过 window.__TAURI__（withGlobalTauri）与 Rust 通信。
//
// 三条约定（踩过坑，别改）：
// 1. listen() 返回 Promise，**必须 await 完**再通知后端"就绪"，否则后端补发的
//    popup-render 会早于注册完成而丢失，弹窗永远停在"翻译中"。
// 2. 每次 render 先复位所有状态，再按 payload 分支。否则上一次留下的 hidden 会残留
//    （典型：先"没取到选中文字"隐藏了原文行，下一次成功也不会再显示）。
// 3. 窗口高度必须由后端设置（ADR-011），这里只能把量到的像素报回去。
const T = window.__TAURI__;
const invoke = T.core.invoke;

const el = (id) => document.getElementById(id);
const card = el("card");
const srcEl = el("src");
const dstEl = el("dst");
const errEl = el("error");
const loadingEl = el("loading");
const actionsEl = el("actions");
const fromLangEl = el("from-lang");

let lastPayload = null;
let hideTimer = null;
let hovering = false;

// 卡片四周给阴影留的边距（px），必须和 popup.css 的 #card margin 一致：
//   margin: 8px 12px 16px  →  上 8、左右 12、下 16
// 窗口尺寸 = 卡片尺寸 + 这些边距，后端 resize_popup 会按这个算原生窗口大小。
const PAD_TOP = 8;
const PAD_X = 12;
const PAD_BOTTOM = 16;

// 内容变化后告诉 Rust 该多大（原生窗口尺寸必须由后端设置，ADR-011）
function reportHeight() {
  const h = Math.ceil(
    card.getBoundingClientRect().height + PAD_TOP + PAD_BOTTOM
  );
  const w = Math.ceil(card.getBoundingClientRect().width + PAD_X * 2);
  invoke("resize_popup", { height: Math.max(60, h), width: Math.max(120, w) }).catch(() => {});
}

function closePopup() {
  invoke("close_popup").catch(() => {});
}

function render(p) {
  lastPayload = p;
  clearTimeout(hideTimer);

  // 先复位所有状态，再按分支渲染
  el("from-row").classList.remove("hidden");
  actionsEl.classList.remove("on");
  el("copy").classList.remove("hidden");
  errEl.textContent = "";
  dstEl.textContent = "";
  loadingEl.classList.add("hidden");
  dstEl.classList.add("hidden");
  srcEl.textContent = p.src || "";
  if (p.src) srcEl.classList.remove("hidden");
  else srcEl.classList.add("hidden");

  if (p.state === "loading") {
    loadingEl.classList.remove("hidden");
    fromLangEl.textContent = "翻译中";
  } else if (p.state === "done") {
    dstEl.textContent = p.dst || "";
    dstEl.classList.remove("hidden");
    // 语种行优先用后端给的中文名（payload.dir），退回代码兜底。
    // 后端在检测出语种后能给准名字，比直接显示 en → zh 好读。
    const d = p.dir || {};
    fromLangEl.textContent = (d.from || p.from || "auto") + " → " + (d.to || p.to || "zh");
    actionsEl.classList.add("on");
    scheduleHide();
  } else if (p.state === "error") {
    errEl.textContent = p.message || "翻译失败";
    fromLangEl.textContent = "出错了";
    // 出错时没有译文可复制，藏掉「复制译文」，只留「重试」
    el("copy").classList.add("hidden");
    actionsEl.classList.add("on");
    // 出错后留久一点，让用户看清原因（用户配置的 2 倍，至少 30s）
    scheduleHide(Math.max(30000, timeoutMs() * 2));
  }

  // 等布局落地再测高度，否则量到的是上一帧的尺寸
  requestAnimationFrame(() => requestAnimationFrame(reportHeight));
}

/**
 * 自动关闭。
 *
 * 不抢焦点的窗口没法靠"失焦"判断用户走神了，所以仍然留这个定时器；
 * 鼠标悬停在卡片上时取消倒计时，用户正在读就不该关。
 * 另有"点击卡片外部关闭"，那是 Rust 侧 30ms 轮询实现的（popup.rs）。
 *
 * 时长来自配置 `popup_timeout_ms`，由后端塞进 payload 的 `timeout` 字段。
 * **不要在这里写死数字**：S2 把它写死成 15000，导致设置页里那个
 * 「弹窗自动关闭」输入框成了摆设（改了没反应）。
 */
function scheduleHide(ms) {
  clearTimeout(hideTimer);
  if (hovering) return;
  hideTimer = setTimeout(closePopup, ms || timeoutMs());
}

/** 读当前 payload 里的超时时长，兜底 15s。 */
function timeoutMs() {
  const t = lastPayload && Number(lastPayload.timeout);
  return Number.isFinite(t) && t >= 1000 ? t : 15000;
}

card.addEventListener("mouseenter", () => {
  hovering = true;
  clearTimeout(hideTimer);
});
card.addEventListener("mouseleave", () => {
  hovering = false;
  scheduleHide();
});

el("close").addEventListener("click", closePopup);

el("copy").addEventListener("click", async () => {
  if (!lastPayload || !lastPayload.dst) return;
  const b = el("copy");
  try {
    await navigator.clipboard.writeText(lastPayload.dst);
    b.textContent = "已复制";
  } catch (e) {
    // 不可聚焦的窗口里 clipboard API 可能被拒，交给 Rust 侧兜底
    invoke("copy_text", { text: lastPayload.dst }).catch(() => {});
    b.textContent = "已复制";
  }
  setTimeout(() => (b.textContent = "复制译文"), 1200);
});

el("retry").addEventListener("click", () => {
  if (!lastPayload) return;
  invoke("retry_translate").catch(() => {});
});

const listening = T.event.listen("popup-render", (e) => render(e.payload));

window.addEventListener("DOMContentLoaded", async () => {
  try {
    await listening;
  } catch (e) {
    // 注册失败也继续走，至少能显示错误卡片
    console.error("popup-render 监听注册失败", e);
  }
  invoke("popup_ready").catch(() => {});
});
