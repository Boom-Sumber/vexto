// 通用框选浮层（S7）：OCR 与截图共用。
// 页面通过 <body data-purpose="ocr|shot"> 声明用途；本文件只按用途切换
// 命令名 / 事件名 / 结果卡文案，框选、坐标、取消逻辑完全共用。
(function () {
  const invoke = window.__TAURI__.core.invoke;
  const el = (id) => document.getElementById(id);
  const purpose =
    document.body.dataset.purpose === "shot"
      ? "shot"
      : document.body.dataset.purpose === "scroll"
        ? "scroll"
        : "ocr";
  const api =
    purpose === "scroll"
      ? { submit: "scroll_start", cancel: "scroll_cancel", event: "scroll:done" }
      : purpose === "shot"
        ? { submit: "shot_submit_region", cancel: "shot_cancel", event: "shot:result" }
        : { submit: "ocr_submit_region", cancel: "ocr_cancel", event: "ocr:result" };

  let translated = "";
  // 截图成功后结果卡上「钉屏 / 预览」要用的文件路径（会话 63）。
  let lastPath = "";
  // 结果卡复用加载卡的锚点：框选落点附近，避免完成后跳到屏幕中上。
  let anchorX = 0, anchorY = 0;
  let closeTimer = 0;

  // 启动：页面就绪即显示（后端 visible(false) 防白屏，这里是 reveal 点）
  if (window.Theme) Theme.apply();
  invoke("window_ready").catch(() => {});

  // Esc 取消：框选中、处理中、出结果后都生效。
  window.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") cancel();
  });

  function cancel() {
    if (closeTimer) clearTimeout(closeTimer);
    invoke(api.cancel).catch(() => {});
  }

  // ── 框选 ──────────────────────────────────────────────────────────────
  let dragging = false;
  let sx = 0, sy = 0;

  el("dim").addEventListener("mousedown", (ev) => {
    if (ev.button !== 0) return;
    dragging = true;
    sx = ev.clientX; sy = ev.clientY;
    const box = el("box");
    box.style.left = sx + "px";
    box.style.top = sy + "px";
    box.style.width = "0px";
    box.style.height = "0px";
    box.classList.add("on");
  });
  window.addEventListener("mousemove", (ev) => {
    if (!dragging) return;
    const box = el("box");
    const x = Math.min(sx, ev.clientX), y = Math.min(sy, ev.clientY);
    box.style.left = x + "px";
    box.style.top = y + "px";
    box.style.width = Math.abs(ev.clientX - sx) + "px";
    box.style.height = Math.abs(ev.clientY - sy) + "px";
  });
  window.addEventListener("mouseup", (ev) => {
    if (!dragging) return;
    dragging = false;
    el("box").classList.remove("on");
    const w = Math.abs(ev.clientX - sx), h = Math.abs(ev.clientY - sy);
    // 太小的框当误触：安静收场
    if (w < 8 || h < 8) { cancel(); return; }

    // 立刻藏暗幕（截屏不能把遮罩截进去），再交给后端。
    // 截图模式**不画「处理中」卡**：卡会出现在选区里被截进图；只在结果回来后再画。
    // 滚动截图同理：选完就把窗口交给后端销毁，进度显示在选区之外的独立小窗。
    el("dim").classList.add("hidden");
    if (purpose === "ocr") {
      showLoading(ev.clientX, ev.clientY);
    } else {
      anchorX = ev.clientX + 14;
      anchorY = ev.clientY + 18;
    }
    invoke(api.submit, {
      x: Math.min(sx, ev.clientX),
      y: Math.min(sy, ev.clientY),
      w, h,
      dpr: window.devicePixelRatio || 1,
    }).catch((e) => showResult({ ok: false, error: String(e) }));
  });

  // ── 卡片 ──────────────────────────────────────────────────────────────
  function placeCard(px, py) {
    const card = el("card");
    card.classList.add("on");
    const r = card.getBoundingClientRect();
    const x = Math.min(px, window.innerWidth - r.width - 12);
    const y = Math.min(py, window.innerHeight - r.height - 12);
    card.style.left = Math.max(12, x) + "px";
    card.style.top = Math.max(12, y) + "px";
  }

  function showLoading(px, py) {
    // 框选落点作为结果卡锚点；+14/+18 与划词弹窗的鼠标偏移一致。
    anchorX = px + 14;
    anchorY = py + 18;
    el("card-tag").textContent = purpose === "shot" ? "截图" : "截图翻译";
    el("card-original").textContent = "";
    el("card-original").style.display = "none";
    el("card-text").className = "loading-dots";
    el("card-text").textContent = purpose === "shot" ? "处理中…" : "识别中…";
    el("card-acts").style.display = "none";
    placeCard(anchorX, anchorY);
  }

  function showResult(p) {
    if (purpose === "shot") return showShotResult(p);
    // 滚动截图的结果由独立进度小窗展示；框选浮层此时已经销毁。
    if (purpose === "scroll") return;
    el("card-original").style.display = "";
    el("card-acts").style.display = "";
    el("card-tag").textContent = p.ok ? "截图翻译 · " + (p.from || "") : "截图翻译";
    if (p.ok) {
      translated = p.translated || "";
      el("card-original").textContent = p.original || "";
      el("card-text").className = "translated";
      el("card-text").textContent = p.translated || "";
      placeCard(anchorX, anchorY);
    } else {
      translated = "";
      el("card-original").textContent = "";
      el("card-text").className = "translated card-error";
      el("card-text").textContent = p.error || "识别失败";
      placeCard(anchorX, anchorY);
    }
  }

  // 截图成功：后端已把图 / 路径交付完，这里只确认一下。
  // 4 秒后自动关窗；鼠标移到卡片上就取消自动关闭，给「钉屏 / 预览」留时间。
  // 失败：留在屏幕上显示原因，用户按 Esc / 关闭。
  function showShotResult(p) {
    el("card-original").style.display = "none";
    el("card-acts").style.display = "";
    el("copyBtn").style.display = "none";
    el("card-tag").textContent = "截图";
    if (p.ok) {
      lastPath = p.path || "";
      el("pinBtn").style.display = lastPath ? "" : "none";
      el("previewBtn").style.display = lastPath ? "" : "none";
      el("card-text").className = "translated";
      el("card-text").textContent = p.message || "已完成";
      const path = el("shotPath");
      if (path) {
        path.textContent = p.path || "";
        path.style.display = p.path ? "block" : "none";
      }
      placeCard(anchorX, anchorY);
      closeTimer = setTimeout(cancel, 4000);
    } else {
      el("card-text").className = "translated card-error";
      el("card-text").textContent = p.error || "截图失败";
      placeCard(anchorX, anchorY);
    }
  }

  window.__TAURI__.event.listen(api.event, (e) => showResult(e.payload || {}));

  el("copyBtn").addEventListener("click", () => {
    if (translated) invoke("copy_text", { text: translated }).catch(() => {});
  });
  el("closeBtn").addEventListener("click", cancel);

  // 钉屏 / 预览：先让后端加图，再关掉浮层（浮层是全屏置顶，会盖住新窗口）。
  function openFromCard(cmd) {
    if (!lastPath) return;
    invoke(cmd, { path: lastPath })
      .then(cancel)
      .catch(cancel);
  }
  el("pinBtn").addEventListener("click", () => openFromCard("pin_add"));
  el("previewBtn").addEventListener("click", () => openFromCard("preview_open"));
  el("card").addEventListener("mouseenter", () => {
    if (closeTimer) {
      clearTimeout(closeTimer);
      closeTimer = 0;
    }
  });
})();
