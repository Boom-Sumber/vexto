// 钉屏（会话 63）：单窗多图。图片走自定义 vextoimg 协议，本页只换 <img> 的
// src，不接收任何图片字节（ADR-040 / ADR-044）。
(function () {
  const invoke = window.__TAURI__.core.invoke;
  const el = (id) => document.getElementById(id);
  let shown = "";

  // visible(false) 防白屏：页面就绪后再显示窗口。
  invoke("window_ready").catch(() => {});

  function show(p) {
    if (!p || !p.url || p.url === shown) return;
    shown = p.url;
    el("img").src = p.url;
    el("idx").textContent = (p.index + 1) + "/" + p.count;
    const many = p.count > 1;
    el("prev").style.visibility = many ? "visible" : "hidden";
    el("next").style.visibility = many ? "visible" : "hidden";
  }

  invoke("pin_state").then(show).catch(() => {});
  window.__TAURI__.event.listen("pin:show", (e) => show(e.payload || {}));

  const step = (delta) => invoke("pin_step", { delta }).catch(() => {});
  el("prev").addEventListener("click", () => step(-1));
  el("next").addEventListener("click", () => step(1));
  el("close").addEventListener("click", () => invoke("pin_close").catch(() => {}));

  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape") invoke("pin_close").catch(() => {});
    else if (e.key === "ArrowLeft") step(-1);
    else if (e.key === "ArrowRight") step(1);
  });
})();
