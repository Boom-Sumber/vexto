// 钉屏（会话 63 / 66）：单窗多图。图片走自定义 vextoimg 协议，本页只换
// <img> 的 src，不接收任何图片字节（ADR-040 / ADR-044）。
// 悬停时显示顶部操作条；滚轮缩放由后端按窗口中心调整窗口大小。
(function () {
  const invoke = window.__TAURI__.core.invoke;
  const el = (id) => document.getElementById(id);
  let zoomBusy = false;

  // visible(false) 防白屏：页面就绪后再显示窗口。
  invoke("window_ready").catch(() => {});

  function setZoom(p) {
    if (p && typeof p.zoom === "number") {
      el("zoom").textContent = Math.round(p.zoom) + "%";
    } else if (p && typeof p.percent === "number") {
      el("zoom").textContent = Math.round(p.percent) + "%";
    }
  }

  function show(p) {
    if (!p || !p.url) return;
    el("img").src = p.url;
    el("idx").textContent = (p.index + 1) + "/" + p.count;
    el("nav").classList.toggle("hidden", p.count <= 1);
    setZoom(p);
  }

  invoke("pin_state").then(show).catch(() => {});
  window.__TAURI__.event.listen("pin:show", (e) => show(e.payload || {}));
  window.__TAURI__.event.listen("pin:zoom", (e) => setZoom(e.payload || {}));

  const step = (delta) => invoke("pin_step", { delta }).catch(() => {});
  el("prev").addEventListener("click", () => step(-1));
  el("next").addEventListener("click", () => step(1));
  el("close").addEventListener("click", () => invoke("pin_close").catch(() => {}));

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
