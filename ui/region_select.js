// 通用框选浮层（S7）：OCR 与截图共用。
//
// - OCR（S5 封版）：实时屏幕 + 暗幕 + 结果卡，行为保持不变。
// - 截图 / 滚动（会话 65 改版）：打开前由后端抓一张全屏快照，页面显示冻结画面；
//   选区内部完全透明，只有绿色边框；鼠标旁有放大取色镜；松手后不保存，
//   选区保留并在下方显示动作栏。
(function () {
  const invoke = window.__TAURI__.core.invoke;
  const el = (id) => document.getElementById(id);
  const purpose =
    document.body.dataset.purpose === "shot"
      ? "shot"
      : document.body.dataset.purpose === "scroll"
        ? "scroll"
        : "ocr";
  const enhanced = purpose !== "ocr";
  const api =
    purpose === "scroll"
      ? { submit: "scroll_start", cancel: "scroll_cancel" }
      : purpose === "shot"
        ? { cancel: "shot_cancel" }
        : { submit: "ocr_submit_region", cancel: "ocr_cancel", event: "ocr:result" };

  let translated = "";
  // 结果卡复用的锚点（OCR）。
  let anchorX = 0, anchorY = 0;
  let closeTimer = 0;

  // ── 框选状态 ──────────────────────────────────────────────────────────
  let dragging = false;
  let sx = 0, sy = 0;

  // ── 截图模式状态 ──────────────────────────────────────────────────────
  let snapReady = false;
  let done = false;
  let busy = false;
  let sel = null;
  let magHex = "";
  let meta = { border: 3, originX: 0, originY: 0, zoom: 8 };
  let accent = "#22c55e";
  let toastTimer = 0;
  let copiedTimer = 0;
  const colorCanvas = document.createElement("canvas");
  colorCanvas.width = 1;
  colorCanvas.height = 1;
  const colorCtx = colorCanvas.getContext("2d", { willReadFrequently: true });
  const magCanvas = el("mag-canvas");
  const magCtx = magCanvas ? magCanvas.getContext("2d") : null;

  // theme.js 用 `const Theme` 定义，不会挂到 window 上；直接判断标识符是否存在。
  if (typeof Theme !== "undefined") Theme.apply();

  if (enhanced) startShot();
  else startOcr();

  // ── 启动 ──────────────────────────────────────────────────────────────
  function startOcr() {
    invoke("window_ready").catch(() => {});
    el("dim").addEventListener("mousedown", onDown);
    window.__TAURI__.event.listen(api.event, (e) => showResult(e.payload || {}));
  }

  function startShot() {
    accent =
      getComputedStyle(document.body).getPropertyValue("--accent").trim() || accent;
    invoke("shot_overlay_state")
      .then((s) => {
        if (!s || !s.url) {
          invoke("window_ready").catch(() => {});
          showToast("截图初始化失败，请按 Esc 重试", true, 0);
          return;
        }
        meta.border = Math.min(5, Math.max(3, Number(s.border) || 3));
        meta.zoom = Math.min(10, Math.max(2, Number(s.zoom) || 8));
        meta.originX = Number(s.originX) || 0;
        meta.originY = Number(s.originY) || 0;
        el("box").style.borderWidth = meta.border + "px";
        const img = el("snap");
        img.crossOrigin = "anonymous";
        img.onload = () => {
          snapReady = true;
          invoke("window_ready").catch(() => {});
        };
        img.onerror = () => {
          invoke("window_ready").catch(() => {});
          showToast("截图快照加载失败，请按 Esc 重试", true, 0);
        };
        img.src = s.url;
      })
      .catch(() => {
        invoke("window_ready").catch(() => {});
        showToast("截图初始化失败，请按 Esc 重试", true, 0);
      });
    el("hit").addEventListener("mousedown", onDown);
    bindToolbar();
  }

  // ── 框选 ──────────────────────────────────────────────────────────────
  function onDown(ev) {
    if (ev.button !== 0 || busy) return;
    if (enhanced) {
      if (!snapReady) return;
      if (done) resetSelection();
      el("dim").classList.add("hidden");
      hideToast();
      hideCard();
      el("toolbar").classList.remove("on");
      el("box").classList.remove("done");
      el("magnifier").classList.add("on");
      done = false;
      sel = null;
      magHex = "";
    }
    dragging = true;
    sx = ev.clientX;
    sy = ev.clientY;
    setBox(sx, sy, 0, 0);
  }

  window.addEventListener("mousemove", (ev) => {
    if (enhanced) updateMagnifier(ev);
    if (!dragging) return;
    const x = Math.min(sx, ev.clientX);
    const y = Math.min(sy, ev.clientY);
    setBox(x, y, Math.abs(ev.clientX - sx), Math.abs(ev.clientY - sy));
  });

  window.addEventListener("mouseup", (ev) => {
    if (!dragging) return;
    dragging = false;
    const w = Math.abs(ev.clientX - sx);
    const h = Math.abs(ev.clientY - sy);
    const rect = {
      x: Math.min(sx, ev.clientX),
      y: Math.min(sy, ev.clientY),
      w,
      h,
    };

    if (purpose === "ocr") {
      el("box").classList.remove("on");
      if (w < 8 || h < 8) {
        cancel();
        return;
      }
      el("dim").classList.add("hidden");
      showLoading(ev.clientX, ev.clientY);
      invoke(api.submit, {
        x: rect.x,
        y: rect.y,
        w,
        h,
        dpr: window.devicePixelRatio || 1,
      }).catch((e) => showResult({ ok: false, error: String(e) }));
      return;
    }

    if (w < 8 || h < 8) {
      resetSelection();
      return;
    }
    setBox(rect.x, rect.y, w, h);
    if (purpose === "scroll") {
      finishScroll(rect);
      return;
    }
    enterDone(rect);
  });

  function setBox(x, y, w, h) {
    const box = el("box");
    box.style.left = x + "px";
    box.style.top = y + "px";
    box.style.width = w + "px";
    box.style.height = h + "px";
    box.classList.add("on");
  }

  function resetSelection() {
    done = false;
    sel = null;
    magHex = "";
    const box = el("box");
    box.classList.remove("on", "done");
    el("toolbar").classList.remove("on");
    el("magnifier").classList.remove("on");
    el("dim").classList.remove("hidden");
    hideToast();
    hideCard();
  }

  // ── 放大取色镜 ────────────────────────────────────────────────────────
  function updateMagnifier(ev) {
    if (!enhanced || done || !snapReady || !magCtx) return;
    const img = el("snap");
    const nw = img.naturalWidth;
    const nh = img.naturalHeight;
    if (!nw || !nh) return;

    const dpr = window.devicePixelRatio || 1;
    const px = Math.round(ev.clientX * dpr);
    const py = Math.round(ev.clientY * dpr);
    const cx = Math.max(0, Math.min(nw - 1, px));
    const cy = Math.max(0, Math.min(nh - 1, py));

    const sample = Math.max(2, Math.round(128 / (meta.zoom || 8)));
    const sw = Math.min(sample, nw);
    const sh = Math.min(sample, nh);
    const sxs = Math.max(0, Math.min(nw - sw, Math.round(cx - sw / 2)));
    const sys = Math.max(0, Math.min(nh - sh, Math.round(cy - sh / 2)));

    magCtx.imageSmoothingEnabled = false;
    magCtx.clearRect(0, 0, 128, 128);
    magCtx.drawImage(img, sxs, sys, sw, sh, 0, 0, 128, 128);
    // 中心十字：白色描边 + 主色调，深浅背景都看得见
    magCtx.strokeStyle = "rgba(255,255,255,.9)";
    magCtx.lineWidth = 3;
    magCtx.beginPath();
    magCtx.moveTo(64, 0);
    magCtx.lineTo(64, 128);
    magCtx.moveTo(0, 64);
    magCtx.lineTo(128, 64);
    magCtx.stroke();
    magCtx.strokeStyle = accent;
    magCtx.lineWidth = 1;
    magCtx.beginPath();
    magCtx.moveTo(64, 0);
    magCtx.lineTo(64, 128);
    magCtx.moveTo(0, 64);
    magCtx.lineTo(128, 64);
    magCtx.stroke();

    let hex = "#000000";
    try {
      colorCtx.clearRect(0, 0, 1, 1);
      colorCtx.drawImage(img, cx, cy, 1, 1, 0, 0, 1, 1);
      const d = colorCtx.getImageData(0, 0, 1, 1).data;
      hex =
        "#" +
        [d[0], d[1], d[2]]
          .map((v) => v.toString(16).padStart(2, "0"))
          .join("")
          .toUpperCase();
    } catch (_) {
      hex = "—";
    }
    magHex = hex;

    el("mag-hex").textContent = hex;
    el("mag-swatch").style.background = hex === "—" ? "transparent" : hex;
    el("mag-pos").textContent =
      meta.originX + px + ", " + (meta.originY + py);

    const mag = el("magnifier");
    mag.classList.add("on");
    const mw = mag.offsetWidth || 140;
    const mh = mag.offsetHeight || 190;
    // 紧贴指针：面板左上角就是鼠标位置；靠近边缘时翻到另一侧。
    let left = ev.clientX;
    let top = ev.clientY;
    if (left + mw + 4 > window.innerWidth) left = ev.clientX - mw;
    if (top + mh + 4 > window.innerHeight) top = ev.clientY - mh;
    mag.style.left = Math.max(0, left) + "px";
    mag.style.top = Math.max(0, top) + "px";
  }

  // ── 选区完成后的动作栏 ────────────────────────────────────────────────
  function enterDone(rect) {
    done = true;
    sel = rect;
    el("box").classList.add("done");
    el("magnifier").classList.remove("on");
    el("toolbar").classList.add("on");
    positionToolbar(rect);
  }

  function positionToolbar(rect) {
    const bar = el("toolbar");
    const bw = bar.offsetWidth || 300;
    const bh = bar.offsetHeight || 44;
    let left = rect.x + rect.w / 2 - bw / 2;
    left = Math.max(12, Math.min(window.innerWidth - bw - 12, left));
    let top = rect.y + rect.h + 12;
    if (top + bh + 12 > window.innerHeight) top = rect.y - bh - 12;
    if (top < 12) top = Math.min(window.innerHeight - bh - 12, rect.y + rect.h + 12);
    bar.style.left = left + "px";
    bar.style.top = Math.max(12, top) + "px";
  }

  function finishScroll(rect) {
    el("hit").style.display = "none";
    el("dim").classList.add("hidden");
    el("box").classList.remove("on");
    el("magnifier").classList.remove("on");
    selectedInvoke(rect);
  }

  function selectedInvoke(rect) {
    invoke(api.submit, {
      x: rect.x,
      y: rect.y,
      w: rect.w,
      h: rect.h,
      dpr: window.devicePixelRatio || 1,
    }).catch(() => cancel());
  }

  function bindToolbar() {
    const bar = el("toolbar");
    if (!bar) return;
    bar.addEventListener("mousedown", (ev) => ev.stopPropagation());
    bar.addEventListener("click", (ev) => {
      const btn = ev.target.closest("[data-act]");
      if (!btn || !sel || busy) return;
      ev.stopPropagation();
      const act = btn.dataset.act;
      if (act === "close") {
        cancel();
        return;
      }
      runAction(act);
    });
  }

  async function runAction(act) {
    if (!sel || busy) return;
    busy = true;
    setBarBusy(true);
    showToast("处理中…", false, 0);
    try {
      const r = await invoke("shot_action", {
        action: act,
        x: sel.x,
        y: sel.y,
        w: sel.w,
        h: sel.h,
        dpr: window.devicePixelRatio || 1,
      });
      if (!r || !r.ok) {
        showToast((r && r.message) || "操作失败", true);
        return;
      }
      showToast(r.message || "完成", false);
      if (act === "pin" || act === "preview" || act === "default") {
        setTimeout(cancel, 80);
      }
    } catch (e) {
      showToast(String(e), true);
    } finally {
      busy = false;
      setBarBusy(false);
    }
  }

  function setBarBusy(b) {
    const bar = el("toolbar");
    if (!bar) return;
    for (const btn of bar.querySelectorAll("button")) btn.disabled = b;
  }

  function hideCard() {
    const c = el("card");
    if (c) c.classList.remove("on");
  }

  // ── 提示 / 取消 ───────────────────────────────────────────────────────
  function showToast(text, isErr, ms) {
    const t = el("toast");
    if (!t) return;
    t.textContent = text;
    t.className = "on" + (isErr ? " err" : "");
    const bar = el("toolbar");
    const br = bar && bar.classList.contains("on") ? bar.getBoundingClientRect() : null;
    const tw = t.offsetWidth;
    const th = t.offsetHeight;
    let left = br
      ? br.left + br.width / 2 - tw / 2
      : window.innerWidth / 2 - tw / 2;
    let top = br ? br.top - th - 8 : window.innerHeight - th - 24;
    if (top < 12) top = br ? br.bottom + 8 : 12;
    t.style.left = Math.max(12, Math.min(window.innerWidth - tw - 12, left)) + "px";
    t.style.top = Math.max(12, top) + "px";
    if (toastTimer) clearTimeout(toastTimer);
    if (ms !== 0) toastTimer = setTimeout(hideToast, ms || 2600);
  }

  function hideToast() {
    const t = el("toast");
    if (t) t.classList.remove("on");
  }

  function cancel() {
    if (closeTimer) clearTimeout(closeTimer);
    invoke(api.cancel).catch(() => {});
  }

  // ── 键盘：Esc 取消；截图模式 Ctrl+C 复制色值 ──────────────────────────
  window.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") {
      cancel();
      return;
    }
    if (
      enhanced &&
      !done &&
      magHex &&
      (ev.ctrlKey || ev.metaKey) &&
      ev.key.toLowerCase() === "c"
    ) {
      ev.preventDefault();
      invoke("copy_text", { text: magHex })
        .then(() => {
          const hint = el("mag-hint");
          if (hint) {
            hint.textContent = "已复制 " + magHex;
            hint.classList.add("copied");
            if (copiedTimer) clearTimeout(copiedTimer);
            copiedTimer = setTimeout(() => {
              hint.textContent = "按 Ctrl+C 复制色值";
              hint.classList.remove("copied");
            }, 1100);
          }
        })
        .catch(() => {});
    }
  });

  document.addEventListener("mouseleave", () => {
    if (enhanced && !done && !dragging) el("magnifier").classList.remove("on");
  });

  // ── OCR 的卡片（保持 S5 原行为） ──────────────────────────────────────
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
    anchorX = px + 14;
    anchorY = py + 18;
    el("card-tag").textContent = "截图翻译";
    el("card-original").textContent = "";
    el("card-original").style.display = "none";
    el("card-text").className = "loading-dots";
    el("card-text").textContent = "识别中…";
    el("card-acts").style.display = "none";
    placeCard(anchorX, anchorY);
  }

  function showResult(p) {
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

  el("copyBtn").addEventListener("click", () => {
    if (translated) invoke("copy_text", { text: translated }).catch(() => {});
  });
  el("closeBtn").addEventListener("click", cancel);
})();
