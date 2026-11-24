// 全屏截图右下角缩略图反馈（S7 会话 72）。
// 倒计时、淡出、悬停暂停/恢复、复制/钉住/预览按钮；图片走 vextoimg 协议。
(function () {
  const invoke = window.__TAURI__.core.invoke;
  const el = (id) => document.getElementById(id);
  let timer = 0;
  let seconds = 4;
  let fadeMs = 400;
  let closing = false;
  let started = false;

  if (typeof Theme !== "undefined") Theme.apply();

  const iconName = { copy: "copy", folder: "folder", pin: "pin", preview: "preview" };
  document.querySelectorAll("[data-act]").forEach((btn) => {
    const name = iconName[btn.dataset.act];
    if (window.Icons && Icons.has(name)) btn.innerHTML = Icons.svg(name, 16);
  });

  function schedule() {
    clearTimeout(timer);
    if (closing) return;
    timer = setTimeout(fade, seconds * 1000);
  }

  function fade() {
    if (closing) return;
    closing = true;
    el("card").classList.add("fade");
    setTimeout(() => invoke("feedback_close").catch(() => {}), fadeMs + 30);
  }

  function show(p) {
    if (!p || !p.url) {
      invoke("feedback_close").catch(() => {});
      return;
    }
    seconds = Math.max(3, Math.min(5, Number(p.seconds) || 4));
    fadeMs = Math.max(200, Math.min(1500, Number(p.fadeMs) || 400));
    closing = false;
    const card = el("card");
    card.classList.remove("fade");
    card.style.transitionDuration = fadeMs + "ms";

    const img = el("shot");
    img.onload = () => {
      invoke("window_ready").catch(() => {});
      if (!started) {
        // 第一次：窗口还是全屏，加载完后收缩到右下角。
        started = true;
        invoke("feedback_start_animation")
          .catch(() => {})
          .finally(schedule);
      } else {
        schedule();
      }
    };
    img.onerror = () => invoke("feedback_close").catch(() => {});
    img.src = p.url;
  }

  invoke("feedback_state")
    .then(show)
    .catch(() => invoke("feedback_close").catch(() => {}));

  window.__TAURI__.event.listen("feedback:state", (e) => show(e.payload || {}));

  document.body.addEventListener("mouseenter", () => clearTimeout(timer));
  document.body.addEventListener("mouseleave", () => schedule());

  document.querySelectorAll("[data-act]").forEach((btn) => {
    btn.addEventListener("click", async (ev) => {
      ev.stopPropagation();
      const act = btn.dataset.act;
      try {
        const r = await invoke("feedback_action", { action: act });
        if (r && r.ok && (act === "pin" || act === "preview" || act === "folder")) {
          closing = true;
          clearTimeout(timer);
          invoke("feedback_close").catch(() => {});
        }
      } catch (_) {
        // 失败不关窗，用户可以重试或直接等它淡出。
      }
    });
  });

  window.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") {
      closing = true;
      clearTimeout(timer);
      invoke("feedback_close").catch(() => {});
    }
  });
})();
