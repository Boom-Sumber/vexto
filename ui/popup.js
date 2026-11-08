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

// 内容变化后告诉 Rust 该多高（原生窗口高度必须由后端设置）
function reportHeight() {
  const h = Math.ceil(
    card.getBoundingClientRect().height +
      (parseFloat(getComputedStyle(card).marginTop) || 0) * 2
  );
  invoke("resize_popup", { height: Math.max(60, h) }).catch(() => {});
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
    // src 为空时（比如重试）也把语种行显示出来，否则卡片第一行是空的
    fromLangEl.textContent = (p.from || "auto") + " → " + (p.to || "zh");
    actionsEl.classList.add("on");
    scheduleHide();
  } else if (p.state === "error") {
    errEl.textContent = p.message || "翻译失败";
    fromLangEl.textContent = "出错了";
    // 出错时没有译文可复制，藏掉「复制译文」，只留「重试」
    el("copy").classList.add("hidden");
    actionsEl.classList.add("on");
    // 出错后留久一点，让用户看清原因
    scheduleHide(30000);
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
 */
function scheduleHide(ms) {
  clearTimeout(hideTimer);
  if (hovering) return;
  hideTimer = setTimeout(closePopup, ms || 15000);
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
