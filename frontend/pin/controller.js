// 钉屏（会话 63 / 66）：单窗多图。图片走自定义 vextoimg 协议，本页只换
// <img> 的 src，不接收任何图片字节（ADR-040 / ADR-044）。
// 悬停时显示顶部操作条；滚轮缩放由后端按窗口中心调整窗口大小。
import { t } from "../i18n/index.js";

export function startPin(root) {
  const invoke = window.__TAURI__.core.invoke;
  const el = (id) => root.querySelector("#" + id);
  let zoomBusy = false;
  let baseW = 0;
  let baseH = 0;
  let zoomPct = 100;
  let selected = false;
  let imgDown = null;
  let resizeDrag = null;
  let pendingZoom = null;
  let zoomSending = false;
  // 每个钉屏窗口的 label；事件里带 label 时只处理自己的，避免多窗口串台。
  let myLabel = "";
  try {
    myLabel = window.__TAURI__.window.getCurrentWindow().label;
  } catch (_) {}

  function mine(p) {
    return !p || !p.label || !myLabel || p.label === myLabel;
  }

  // visible(false) 防白屏：页面就绪后再显示窗口。
  invoke("window_ready").catch(() => {});

  // 窗口在屏幕内按缩放变大；超过屏幕后窗口不再变大，靠这里把 <img> 缩放到
  // 目标尺寸（超出部分裁切），保证百分比与画面一致。
  function applyZoom(p) {
    if (!mine(p)) return;
    if (typeof p.zoom === "number") zoomPct = p.zoom;
    else if (typeof p.percent === "number") zoomPct = p.percent;
    if (typeof p.baseW === "number" && p.baseW > 0) baseW = p.baseW;
    if (typeof p.baseH === "number" && p.baseH > 0) baseH = p.baseH;
    el("zoom").textContent = Math.round(zoomPct) + "%";
    if (!baseW || !baseH) return;
    const dpr = window.devicePixelRatio || 1;
    const im = el("img");
    im.style.width = (baseW * (zoomPct / 100)) / dpr + "px";
    im.style.height = (baseH * (zoomPct / 100)) / dpr + "px";
    requestAnimationFrame(positionFrame);
  }

  function positionFrame() {
    const frame = el("frame");
    if (!selected || !frame) return;
    const r = el("img").getBoundingClientRect();
    frame.style.left = r.left + "px";
    frame.style.top = r.top + "px";
    frame.style.width = r.width + "px";
    frame.style.height = r.height + "px";
    frame.classList.remove("hidden");
  }

  function setSelected(next) {
    selected = !!next;
    document.body.classList.toggle("sel", selected);
    const frame = el("frame");
    if (!frame) return;
    if (selected) positionFrame();
    else frame.classList.add("hidden");
  }

  function applyLocalZoom(percent) {
    if (!baseW || !baseH) return;
    const dpr = window.devicePixelRatio || 1;
    const im = el("img");
    im.style.width = (baseW * (percent / 100)) / dpr + "px";
    im.style.height = (baseH * (percent / 100)) / dpr + "px";
    el("zoom").textContent = Math.round(percent) + "%";
    positionFrame();
  }

  function queueZoom(percent) {
    pendingZoom = percent;
    if (zoomSending) return;
    zoomSending = true;
    setTimeout(() => {
      const target = pendingZoom;
      pendingZoom = null;
      zoomSending = false;
      if (target != null) {
        invoke("pin_set_zoom", { zoom: target }).catch(() => {});
      }
    }, 50);
  }

  function beginResize(ev) {
    if (!selected) return;
    ev.preventDefault();
    ev.stopPropagation();
    const r = el("img").getBoundingClientRect();
    const cx = r.left + r.width / 2;
    const cy = r.top + r.height / 2;
    resizeDrag = {
      cx,
      cy,
      startDist: Math.max(20, Math.hypot(ev.clientX - cx, ev.clientY - cy)),
      startZoom: zoomPct,
      last: zoomPct,
    };
    window.addEventListener("pointermove", onResizeMove);
    window.addEventListener("pointerup", endResize, { once: true });
  }

  function onResizeMove(ev) {
    if (!resizeDrag) return;
    const dist = Math.hypot(ev.clientX - resizeDrag.cx, ev.clientY - resizeDrag.cy);
    const target = Math.max(
      20,
      Math.min(400, resizeDrag.startZoom * (dist / resizeDrag.startDist))
    );
    const rounded = Math.round(target);
    if (Math.abs(rounded - resizeDrag.last) < 1) return;
    resizeDrag.last = rounded;
    zoomPct = rounded;
    applyLocalZoom(rounded);
    queueZoom(rounded);
  }

  function endResize() {
    if (!resizeDrag) return;
    const target = resizeDrag.last;
    resizeDrag = null;
    window.removeEventListener("pointermove", onResizeMove);
    queueZoom(target);
  }

  function show(p) {
    if (!p || !p.url || !mine(p)) return;
    setSelected(false);
    el("img").src = p.url;
    el("idx").textContent = (p.index + 1) + "/" + p.count;
    el("nav").classList.toggle("hidden", p.count <= 1);
    applyZoom(p);
  }

  invoke("pin_state").then(show).catch(() => {});
  window.__TAURI__.event.listen("pin:show", (e) => show(e.payload || {}));
  window.__TAURI__.event.listen("pin:zoom", (e) => applyZoom(e.payload || {}));

  el("img").addEventListener("pointerdown", (ev) => {
    imgDown = { x: ev.clientX, y: ev.clientY };
  });
  el("img").addEventListener("pointerup", (ev) => {
    const moved = imgDown
      ? Math.hypot(ev.clientX - imgDown.x, ev.clientY - imgDown.y)
      : 99;
    imgDown = null;
    if (moved < 5) setSelected(!selected);
  });
  el("img").addEventListener("click", (ev) => ev.stopPropagation());
  el("img").addEventListener("load", () => requestAnimationFrame(positionFrame));
  el("stage").addEventListener("click", (ev) => {
    if (ev.target === el("stage")) setSelected(false);
  });
  for (const h of root.querySelectorAll("#frame .ph")) {
    h.addEventListener("pointerdown", beginResize);
  }
  window.addEventListener("resize", () => requestAnimationFrame(positionFrame));

  const step = (delta) => invoke("pin_step", { delta }).catch(() => {});
  el("prev").addEventListener("click", () => step(-1));
  el("next").addEventListener("click", () => step(1));
  el("close").addEventListener("click", () => invoke("pin_close").catch(() => {}));

  let msgTimer = null;
  function flash(text, bad) {
    const box = el("msg");
    box.textContent = text || "";
    box.className = text ? "on" + (bad ? " bad" : "") : "";
    clearTimeout(msgTimer);
    if (text) msgTimer = setTimeout(() => (box.className = ""), 3000);
  }

  el("preview").addEventListener("click", () => {
    invoke("pin_preview")
      .then((r) => {
        if (r && r.ok) {
          flash(r.message || t("pin.previewOpened"));
          setTimeout(() => invoke("pin_close").catch(() => {}), 80);
        } else {
          flash((r && r.message) || t("pin.previewFailed"), true);
        }
      })
      .catch((e) => flash(String(e), true));
  });

  el("copy").addEventListener("click", () => {
    invoke("pin_copy")
      .then((r) => {
        if (!r || !r.ok) {
          flash((r && r.message) || t("pin.copyFailed"), true);
          return;
        }
        const btn = el("copy");
        btn.classList.add("copied");
        setTimeout(() => btn.classList.remove("copied"), 700);
        flash(t("pin.copiedImage"));
      })
      .catch((e) => flash(String(e), true));
  });

  el("reset").addEventListener("click", () => {
    invoke("pin_zoom", { delta: 0, reset: true }).catch(() => {});
  });

  window.addEventListener(
    "wheel",
    (ev) => {
      ev.preventDefault();
      if (zoomBusy) return;
      zoomBusy = true;
      invoke("pin_zoom", { delta: ev.deltaY < 0 ? 1 : -1, reset: false })
        .catch(() => {})
        .finally(() => {
          zoomBusy = false;
        });
    },
    { passive: false }
  );

  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape") invoke("pin_close").catch(() => {});
    else if (e.key === "ArrowLeft") step(-1);
    else if (e.key === "ArrowRight") step(1);
  });
}
