// 钉屏（会话 63 / 66）：单窗多图。图片走自定义 vextoimg 协议，本页只换
// <img> 的 src，不接收任何图片字节（ADR-040 / ADR-044）。
// 悬停时显示顶部操作条；滚轮缩放由后端按窗口中心调整窗口大小。
(function () {
  const invoke = window.__TAURI__.core.invoke;
  const el = (id) => document.getElementById(id);
  let zoomBusy = false;
  let baseW = 0;
  let baseH = 0;
  let zoomPct = 100;
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
  }

  function show(p) {
    if (!p || !p.url || !mine(p)) return;
    el("img").src = p.url;
    el("idx").textContent = (p.index + 1) + "/" + p.count;
    el("nav").classList.toggle("hidden", p.count <= 1);
    applyZoom(p);
  }

  invoke("pin_state").then(show).catch(() => {});
  window.__TAURI__.event.listen("pin:show", (e) => show(e.payload || {}));
  window.__TAURI__.event.listen("pin:zoom", (e) => applyZoom(e.payload || {}));

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

  el("saveas").addEventListener("click", () => {
    invoke("pin_save_as")
      .then((r) => {
        if (r && r.ok) flash(r.message || "已保存");
        else flash((r && r.message) || "保存失败", true);
      })
      .catch((e) => flash(String(e), true));
  });

  el("preview").addEventListener("click", () => {
    invoke("pin_preview")
      .then((r) => {
        if (r && r.ok) flash(r.message || "已打开预览");
        else flash((r && r.message) || "打开预览失败", true);
      })
      .catch((e) => flash(String(e), true));
  });

  el("copy").addEventListener("click", () => {
    invoke("pin_copy")
      .then((r) => {
        if (!r || !r.ok) return;
        const btn = el("copy");
        btn.classList.add("copied");
        setTimeout(() => btn.classList.remove("copied"), 700);
      })
      .catch(() => {});
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
})();
