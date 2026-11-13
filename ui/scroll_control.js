// 滚动截图进度窗（S7 会话 62）。
// 后端在工作线程跑主循环，这里只显示进度与三态结果；「停止」写一个
// 原子标志，主循环在下一拍看到就收尾。
(function () {
  const invoke = window.__TAURI__.core.invoke;
  const el = (id) => document.getElementById(id);
  let finished = false;
  let closeTimer = 0;

  if (window.Theme) Theme.apply();

  window.__TAURI__.event.listen("scroll:progress", (e) => {
    if (finished) return;
    const p = e.payload || {};
    const frames = p.frames || 0;
    const height = p.height || 0;
    el("scr-line").textContent =
      p.stage === "writing"
        ? "正在写盘…"
        : `已拼接 ${frames} 帧 · ${height} px`;
    // 帧数上限是 100，用它当粗略进度；总高上限另看 height。
    const pct = Math.min(100, Math.max(4, Math.round((frames / 100) * 100)));
    el("scr-fill").style.width = pct + "%";
  });

  window.__TAURI__.event.listen("scroll:done", (e) => {
    if (finished) return;
    finished = true;
    const p = e.payload || {};
    const status = p.status || "failed";
    const badge = el("scr-badge");
    badge.textContent =
      status === "success" ? "完成" : status === "partial" ? "部分成功" : "失败";
    badge.className = "scr-badge " + status;
    el("scr-line").textContent = p.message || "已完成";
    el("scr-fill").style.width = status === "failed" ? "0%" : "100%";
    el("scr-stop").hidden = true;
    const close = el("scr-close");
    close.hidden = false;
    // 成功才自动关；部分成功 / 失败留着让用户看清原因。
    if (status === "success") {
      closeTimer = setTimeout(() => invoke("scroll_close").catch(() => {}), 6000);
    }
  });

  el("scr-stop").addEventListener("click", () => {
    el("scr-stop").disabled = true;
    el("scr-line").textContent = "正在停止…";
    invoke("scroll_stop").catch(() => {});
  });

  el("scr-close").addEventListener("click", () => {
    if (closeTimer) clearTimeout(closeTimer);
    invoke("scroll_close").catch(() => {});
  });

  window.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") {
      if (closeTimer) clearTimeout(closeTimer);
      invoke("scroll_close").catch(() => {});
    }
  });
})();
