// 截图预览（会话 63）：图片走自定义 vextoimg 协议，裁剪由后端按像素行流式
// 完成。本页只负责显示与收集 head/tail，不接收图片字节（ADR-040 / ADR-044）。
//
// 会话 91 起增加标注：画布与图片重合，标注只在导出前合成一次 PNG，再按
// 原始字节提交给后端替换预览源（ADR-068）；关掉标注工具时画布点击穿透。
//
// 会话 92：工具气泡、实心/空心、锥形箭头、图形选中/移动/缩放、文字动态输入与
// 双击编辑。
(function () {
  const invoke = window.__TAURI__.core.invoke;
  const el = (id) => document.getElementById(id);
  const img = el("pv-img");
  const headEl = el("pv-head");
  const tailEl = el("pv-tail");
  const annot = el("pv-annot");
  const annotCtx = annot ? annot.getContext("2d") : null;
  const editBar = el("pv-edit-bar");
  const hintEl = el("pv-edit-hint");
  const pop = el("pv-tool-pop");
  const popBody = el("pv-pop-body");

  // theme.js 用 `const Theme` 定义，不挂 window；预览窗之前漏了这一步，始终暗色。
  if (typeof Theme !== "undefined") Theme.apply();

  let state = { path: "", width: 0, height: 0 };
  let head = 0;
  let tail = 0;
  let busy = false;
  let zoom = 1;
  let dpr = window.devicePixelRatio || 1;

  // ── 标注状态 ─────────────────────────────────────────────────────────
  const MAX_EDIT_PIXELS = 16000000;
  const WIDTHS = [2, 4, 8];
  const TEXT_SIZES = [14, 20, 28];
  const HANDLE_CURSORS = {
    nw: "nwse-resize",
    n: "ns-resize",
    ne: "nesw-resize",
    e: "ew-resize",
    se: "nwse-resize",
    s: "ns-resize",
    sw: "nesw-resize",
    w: "ew-resize",
    p1: "crosshair",
    p2: "crosshair",
  };
  const POP_TOOLS = new Set(["rect", "ellipse", "arrow", "pen", "mosaic", "text"]);
  const settings = {
    rect: { level: 1, fill: false },
    ellipse: { level: 1, fill: false },
    arrow: { level: 1 },
    pen: { level: 1 },
    mosaic: { level: 1 },
    text: { level: 1 },
  };

  let tool = "";
  let color = "#f0566d";
  let ops = [];
  let history = [[]];
  let hp = 0;
  let nextId = 1;
  let selectedId = null;
  let draft = null;
  let dragId = null;
  let interaction = null;
  let textInput = null;
  let editingId = null;
  let renderQueued = false;
  let imgReady = false;
  let popOpen = false;
  let popAnchor = null;
  let accent = "#7c5cff";
  const mosaicScratch = document.createElement("canvas");
  const measureCanvas = document.createElement("canvas");
  const measureCtx = measureCanvas.getContext("2d");
  const textFont = '"Microsoft YaHei", "Segoe UI", sans-serif';

  // 马赛克画笔光标：半透明小圆球跟随指针。
  const brush = document.createElement("div");
  brush.className = "pv-brush-cursor";
  document.body.appendChild(brush);

  function readAccent() {
    return (
      getComputedStyle(document.body).getPropertyValue("--accent").trim() ||
      "#7c5cff"
    );
  }
  accent = readAccent();
  document.addEventListener("vexto:theme", () => {
    accent = readAccent();
    renderAnnot();
  });

  if (typeof Icons !== "undefined") {
    el("pv-tool-move").innerHTML = Icons.svg("pointer", 20);
    const ICONS = {
      rect: "rect",
      ellipse: "ellipse",
      arrow: "arrow-up-right",
      pen: "pen",
      text: "text",
      mosaic: "mosaic",
    };
    for (const [name, icon] of Object.entries(ICONS)) {
      const btn = editBar && editBar.querySelector('[data-tool="' + name + '"]');
      if (btn) btn.innerHTML = Icons.svg(icon, 20);
    }
    el("pv-undo").innerHTML = Icons.svg("undo", 20);
    el("pv-redo").innerHTML = Icons.svg("redo", 20);
    el("pv-clear").innerHTML = Icons.svg("trash", 20);
  }

  // 首屏状态拿到后再显示窗口（titlebar.js 的 reveal）；1.5s 兜底。

  function labels() {
    el("pv-head-v").textContent = head + " px";
    el("pv-tail-v").textContent = tail + " px";
  }

  function imageEditable() {
    const px = (state.width || 0) * (state.height || 0);
    return !!state.url && px > 0 && px <= MAX_EDIT_PIXELS;
  }

  function editable() {
    return !busy && imageEditable();
  }

  function applyState(p) {
    if (!p || !p.url) {
      el("pv-info").textContent = "没有可预览的图片";
      state = { path: "", width: 0, height: 0 };
      resetAnnotations();
      updateEditAvailability();
      return;
    }
    state = { url: p.url, width: p.width || 0, height: p.height || 0 };
    head = 0;
    tail = 0;
    zoom = 1;
    headEl.max = Math.max(0, state.height - 1);
    tailEl.max = Math.max(0, state.height - 1);
    headEl.value = 0;
    tailEl.value = 0;
    labels();
    el("pv-info").textContent = state.width + " × " + state.height + " px";
    el("pv-msg").textContent = "";
    el("pv-ocr-box").hidden = true;
    el("pv-translation").hidden = true;
    el("pv-translation-text").textContent = "";
    resetAnnotations();
    updateEditAvailability();
    // 标注导出需要读回画布像素；必须带 CORS 请求头，否则 canvas 会被标记为
    // tainted，toBlob 直接抛 SecurityError。vextoimg 已返回 ACAO: *。
    img.crossOrigin = "anonymous";
    imgReady = false;
    img.onload = () => {
      imgReady = true;
      applyZoom();
      updateBands();
      sizeAnnot();
      renderAnnot();
    };
    img.onerror = () => {
      imgReady = false;
    };
    img.src = state.url;
    applyZoom();
    sizeAnnot();
  }

  function clamp() {
    const h = state.height || 1;
    head = Math.max(0, Math.min(Number(headEl.value) || 0, h - 1));
    tail = Math.max(0, Math.min(Number(tailEl.value) || 0, h - 1 - head));
    headEl.value = head;
    tailEl.value = tail;
    labels();
    updateBands();
  }

  function updateBands() {
    if (!state.height || !img.clientHeight) return;
    const s = img.clientHeight / state.height;
    el("pv-band-top").style.height = Math.round(head * s) + "px";
    el("pv-band-bottom").style.height = Math.round(tail * s) + "px";
  }

  // 预览图默认 1:1（按物理像素），不自动缩放；超出视口时由 .pv-viewport 滚动。
  function applyZoom() {
    if (!state.width || !state.height) return;
    img.style.width = Math.round((state.width / dpr) * zoom) + "px";
    img.style.height = Math.round((state.height / dpr) * zoom) + "px";
    syncAnnotCss();
    setTimeout(updateBands, 0);
  }

  function setZoom(next) {
    zoom = Math.max(0.1, Math.min(4, next));
    applyZoom();
  }

  function msg(text) {
    el("pv-msg").textContent = text || "";
  }

  function setBusy(b) {
    busy = b;
    for (const id of ["pv-save", "pv-copy", "pv-pin", "pv-ocr", "pv-translate", "pv-copy-text"]) {
      el(id).disabled = b;
    }
    if (editBar) {
      for (const node of editBar.querySelectorAll("button, select")) node.disabled = b;
    }
    if (b) closePop();
    if (!b) updateEditAvailability();
  }

  // ── 画布尺寸与可用性 ─────────────────────────────────────────────────
  function sizeAnnot() {
    if (!annot) return;
    if (!state.width || !state.height || !editable()) {
      annot.hidden = true;
      annot.width = 1;
      annot.height = 1;
      return;
    }
    annot.hidden = false;
    if (annot.width !== state.width || annot.height !== state.height) {
      annot.width = state.width;
      annot.height = state.height;
    }
    syncAnnotCss();
    renderAnnot();
  }

  function syncAnnotCss() {
    if (!annot || annot.hidden) return;
    annot.style.width = (img.clientWidth || 0) + "px";
    annot.style.height = (img.clientHeight || 0) + "px";
  }

  function updateEditAvailability() {
    const ok = editable();
    if (editBar) {
      for (const node of editBar.querySelectorAll("[data-tool], .pv-color")) {
        node.disabled = !ok;
      }
    }
    if (!ok) setTool("");
    updateUndoButtons();
    sizeAnnot();
    if (hintEl) {
      if (state.url && state.width && state.height && !editable() && !busy) {
        hintEl.textContent =
          "图片过大（" + state.width + "×" + state.height + "），暂不支持标注";
      } else if (tool) {
        hintEl.textContent = "拖动绘制；点已有图形可移动 / 缩放";
      } else {
        hintEl.textContent = "点图形可移动 / 缩放；选工具后拖动绘制";
      }
    }
  }

  function updateUndoButtons() {
    const ok = editable();
    el("pv-undo").disabled = !ok || hp <= 0;
    el("pv-redo").disabled = !ok || hp >= history.length - 1;
    el("pv-clear").disabled = !ok || ops.length === 0;
  }

  function resetAnnotations() {
    cancelTextInput();
    closePop();
    hideBrush();
    ops = [];
    history = [[]];
    hp = 0;
    selectedId = null;
    draft = null;
    dragId = null;
    interaction = null;
    editingId = null;
    updateUndoButtons();
    renderAnnot();
  }

  function commitSnapshot() {
    history = history.slice(0, hp + 1);
    history.push(structuredClone(ops));
    hp = history.length - 1;
    updateUndoButtons();
  }

  function findOp(id) {
    return ops.find((op) => op.id === id) || null;
  }

  function undoAnnot() {
    if (!editable() || hp <= 0) return;
    cancelTextInput();
    draft = null;
    interaction = null;
    dragId = null;
    hp -= 1;
    ops = structuredClone(history[hp]);
    if (!findOp(selectedId)) selectedId = null;
    renderAnnot();
    updateUndoButtons();
  }

  function redoAnnot() {
    if (!editable() || hp >= history.length - 1) return;
    cancelTextInput();
    draft = null;
    interaction = null;
    dragId = null;
    hp += 1;
    ops = structuredClone(history[hp]);
    if (!findOp(selectedId)) selectedId = null;
    renderAnnot();
    updateUndoButtons();
  }

  function clearAnnot() {
    if (!editable() || ops.length === 0) return;
    cancelTextInput();
    ops = [];
    selectedId = null;
    commitSnapshot();
    renderAnnot();
  }

  function setTool(next) {
    if (dragId !== null) {
      draft = null;
      dragId = null;
      interaction = null;
    }
    tool = editable() ? next || "" : "";
    if (editBar) {
      for (const btn of editBar.querySelectorAll(".pv-tool")) {
        btn.classList.toggle("on", (btn.dataset.tool || "") === tool);
      }
    }
    if (annot) {
      annot.classList.toggle("drawing", !!tool);
      annot.classList.toggle("textmode", tool === "text");
      annot.style.cursor = tool === "text" ? "text" : tool ? "crosshair" : "default";
    }
    if (tool !== "mosaic") hideBrush();
    if (!tool) closePop();
    if (hintEl) {
      hintEl.textContent = tool
        ? "拖动绘制；点已有图形可移动 / 缩放"
        : "点图形可移动 / 缩放；选工具后拖动绘制";
    }
    if (!tool) cancelTextInput();
  }

  // ── 工具气泡 ─────────────────────────────────────────────────────────
  function closePop() {
    if (!pop) return;
    pop.hidden = true;
    popOpen = false;
    popAnchor = null;
  }

  function openPop(btn) {
    if (!pop || !btn || !editable()) return;
    popAnchor = btn;
    popOpen = true;
    pop.hidden = false;
    renderPop();
    positionPop();
  }

  function positionPop() {
    if (!popOpen || !pop || !popAnchor) return;
    const r = popAnchor.getBoundingClientRect();
    const pw = pop.offsetWidth || 120;
    const ph = pop.offsetHeight || 40;
    let left = r.left + r.width / 2 - pw / 2;
    left = Math.max(8, Math.min(window.innerWidth - pw - 8, left));
    let top = r.bottom + 9;
    if (top + ph > window.innerHeight - 8) top = Math.max(8, r.top - ph - 9);
    pop.style.left = Math.round(left) + "px";
    pop.style.top = Math.round(top) + "px";
    pop.style.setProperty("--arrow-left", Math.round(r.left + r.width / 2 - left - 4) + "px");
  }

  function makeDot(level) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "pop-dot";
    b.title = ["细", "中", "粗"][level];
    b.style.setProperty("--dot", [5, 8, 11][level] + "px");
    b.disabled = !editable();
    const cfg = settings[tool];
    const solidShape = (tool === "rect" || tool === "ellipse") && cfg.fill;
    b.classList.toggle("on", !solidShape && cfg.level === level);
    if (solidShape) b.disabled = true;
    b.addEventListener("click", () => {
      if (b.disabled) return;
      cfg.level = level;
      renderPop();
    });
    return b;
  }

  function makeFill() {
    const cfg = settings[tool];
    const b = document.createElement("button");
    b.type = "button";
    b.className = "pop-fill" + (cfg.fill ? " on" : "");
    b.title = cfg.fill ? "实心（点击切换空心）" : "空心（点击切换实心）";
    b.innerHTML =
      typeof Icons !== "undefined"
        ? Icons.svg(cfg.fill ? "square-solid" : "square-hollow", 18)
        : "";
    b.addEventListener("click", () => {
      cfg.fill = !cfg.fill;
      renderPop();
    });
    return b;
  }

  function makeTextSize(level) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "pop-size" + (settings.text.level === level ? " on" : "");
    b.textContent = ["小", "中", "大"][level];
    b.addEventListener("click", () => {
      settings.text.level = level;
      renderPop();
    });
    return b;
  }

  function renderPop() {
    if (!popBody || !POP_TOOLS.has(tool)) return;
    popBody.textContent = "";
    if (tool === "text") {
      for (let i = 0; i < 3; i++) popBody.appendChild(makeTextSize(i));
      return;
    }
    for (let i = 0; i < 3; i++) popBody.appendChild(makeDot(i));
    if (tool === "rect" || tool === "ellipse") {
      const sep = document.createElement("span");
      sep.className = "pv-pop-sep";
      popBody.appendChild(sep);
      popBody.appendChild(makeFill());
    }
  }

  // ── 文字测量与图形边界 ───────────────────────────────────────────────
  function setTextFont(ctx, size) {
    ctx.font = "600 " + size + "px " + textFont;
  }

  function measureText(text, size) {
    if (!measureCtx) return { w: size, h: size * 1.2 };
    setTextFont(measureCtx, size);
    return { w: Math.ceil(measureCtx.measureText(text).width) + 2, h: Math.ceil(size * 1.2) };
  }

  function refreshTextMetrics(op) {
    const m = measureText(op.text, op.size);
    op.w = m.w;
    op.h = m.h;
  }

  function opBounds(op) {
    if (op.tool === "rect" || op.tool === "ellipse" || op.tool === "text") {
      return { x: op.x, y: op.y, w: op.w || 0, h: op.h || 0 };
    }
    if (op.tool === "arrow") {
      const pad = Math.max(10, (op.width || 2) * 2.5);
      return {
        x: Math.min(op.x1, op.x2) - pad,
        y: Math.min(op.y1, op.y2) - pad,
        w: Math.abs(op.x2 - op.x1) + pad * 2,
        h: Math.abs(op.y2 - op.y1) + pad * 2,
      };
    }
    const pts = op.points || [];
    let x1 = Infinity, y1 = Infinity, x2 = -Infinity, y2 = -Infinity;
    for (const p of pts) {
      x1 = Math.min(x1, p.x);
      y1 = Math.min(y1, p.y);
      x2 = Math.max(x2, p.x);
      y2 = Math.max(y2, p.y);
    }
    if (!isFinite(x1)) return { x: 0, y: 0, w: 0, h: 0 };
    const pad = Math.max(6, (op.width || 2) / 2);
    return { x: x1 - pad, y: y1 - pad, w: x2 - x1 + pad * 2, h: y2 - y1 + pad * 2 };
  }

  function distToSegment(p, a, b) {
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const len2 = dx * dx + dy * dy;
    if (len2 === 0) return Math.hypot(p.x - a.x, p.y - a.y);
    let t = ((p.x - a.x) * dx + (p.y - a.y) * dy) / len2;
    t = Math.max(0, Math.min(1, t));
    return Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy));
  }

  function pointInBounds(p, b) {
    return p.x >= b.x && p.x <= b.x + b.w && p.y >= b.y && p.y <= b.y + b.h;
  }

  function hitOp(op, p) {
    const tol = Math.max(8, (op.width || 2) * 1.5);
    if (op.tool === "mosaic") {
      // 马赛克是像素化结果，不作为可选中/移动的图形。
      return false;
    }
    if (op.tool === "text") {
      return pointInBounds(p, opBounds(op));
    }
    if (op.tool === "rect") {
      const b = opBounds(op);
      if (op.fill) return pointInBounds(p, b);
      const inside = pointInBounds(p, b);
      if (inside) {
        const d = Math.min(p.x - b.x, b.x + b.w - p.x, p.y - b.y, b.y + b.h - p.y);
        return d <= tol;
      }
      const dx = Math.max(b.x - p.x, 0, p.x - (b.x + b.w));
      const dy = Math.max(b.y - p.y, 0, p.y - (b.y + b.h));
      return Math.hypot(dx, dy) <= tol;
    }
    if (op.tool === "ellipse") {
      const b = opBounds(op);
      const rx = Math.max(1, b.w / 2);
      const ry = Math.max(1, b.h / 2);
      const nx = (p.x - (b.x + rx)) / rx;
      const ny = (p.y - (b.y + ry)) / ry;
      const v = Math.sqrt(nx * nx + ny * ny);
      if (op.fill) return v <= 1;
      return Math.abs(v - 1) * Math.min(rx, ry) <= tol;
    }
    if (op.tool === "arrow") {
      return (
        distToSegment(
          p,
          { x: op.x1, y: op.y1 },
          { x: op.x2, y: op.y2 }
        ) <= Math.max(10, (op.width || 2) * 2.5)
      );
    }
    const pts = op.points || [];
    if (pts.length === 0) return false;
    if (pts.length === 1) return Math.hypot(p.x - pts[0].x, p.y - pts[0].y) <= tol;
    for (let i = 1; i < pts.length; i++) {
      if (distToSegment(p, pts[i - 1], pts[i]) <= tol) return true;
    }
    return false;
  }

  function topOpAt(p) {
    for (let i = ops.length - 1; i >= 0; i--) {
      const op = ops[i];
      if (op.id === editingId) continue;
      if (hitOp(op, p)) return op;
    }
    return null;
  }

  function handlesOf(op) {
    if (op.tool === "rect" || op.tool === "ellipse") {
      const { x, y, w, h } = op;
      return [
        { key: "nw", x, y },
        { key: "n", x: x + w / 2, y },
        { key: "ne", x: x + w, y },
        { key: "e", x: x + w, y: y + h / 2 },
        { key: "se", x: x + w, y: y + h },
        { key: "s", x: x + w / 2, y: y + h },
        { key: "sw", x, y: y + h },
        { key: "w", x, y: y + h / 2 },
      ];
    }
    if (op.tool === "arrow") {
      return [
        { key: "p1", x: op.x1, y: op.y1 },
        { key: "p2", x: op.x2, y: op.y2 },
      ];
    }
    return [];
  }

  function handleAt(op, p) {
    if (!op) return null;
    const scale = annot.clientWidth ? annot.clientWidth / annot.width : 1;
    const tol = 10 / (scale || 1);
    for (const h of handlesOf(op)) {
      if (Math.hypot(p.x - h.x, p.y - h.y) <= tol) return h;
    }
    return null;
  }

  // ── 绘制 ─────────────────────────────────────────────────────────────
  function renderAnnot() {
    if (!annotCtx || !annot || annot.hidden || !state.width || !state.height) return;
    const w = annot.width;
    const h = annot.height;
    annotCtx.clearRect(0, 0, w, h);
    if (!imgReady || !img.complete || !img.naturalWidth) return;
    annotCtx.drawImage(img, 0, 0, w, h);
    for (const op of ops) {
      if (op.id === editingId) continue;
      drawOp(annotCtx, op);
    }
    if (draft) drawOp(annotCtx, draft);
    if (selectedId) drawSelection(annotCtx);
    if (draft) drawDraftHelpers(annotCtx);
  }

  function scheduleRender() {
    if (renderQueued) return;
    renderQueued = true;
    requestAnimationFrame(() => {
      renderQueued = false;
      renderAnnot();
    });
  }

  function drawOp(ctx, op) {
    ctx.save();
    ctx.strokeStyle = op.color;
    ctx.fillStyle = op.color;
    ctx.lineWidth = Math.max(1, op.width || 2);
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    switch (op.tool) {
      case "rect":
        if (op.w > 0 && op.h > 0) {
          if (op.fill) ctx.fillRect(op.x, op.y, op.w, op.h);
          else ctx.strokeRect(op.x, op.y, op.w, op.h);
        }
        break;
      case "ellipse":
        if (op.w > 0 && op.h > 0) {
          ctx.beginPath();
          ctx.ellipse(
            op.x + op.w / 2,
            op.y + op.h / 2,
            Math.max(op.w / 2, 0.5),
            Math.max(op.h / 2, 0.5),
            0,
            0,
            Math.PI * 2
          );
          if (op.fill) ctx.fill();
          else ctx.stroke();
        }
        break;
      case "arrow":
        drawArrow(ctx, op);
        break;
      case "pen":
        drawPen(ctx, op);
        break;
      case "mosaic":
        drawMosaic(ctx, op);
        break;
      case "text":
        drawText(ctx, op);
        break;
      default:
        break;
    }
    ctx.restore();
  }

  function drawArrow(ctx, op) {
    const dx = op.x2 - op.x1;
    const dy = op.y2 - op.y1;
    const len = Math.hypot(dx, dy);
    if (len < 1) return;
    const ux = dx / len;
    const uy = dy / len;
    const px = -uy;
    const py = ux;
    const w = Math.max(1, op.width || 2);
    const headLen = Math.min(len * 0.45, Math.max(14, w * 4.2));
    // 三角形长宽比 1:0.8：半宽 = 长度 × 0.4。
    const headHalf = headLen * 0.4;
    const shaftEnd = Math.max(0, len - headLen);
    // 起点固定 2px（物理 2×dpr），与线条粗细无关。
    const startHalf = Math.max(0.5, dpr);
    const endHalf = Math.max(0.8, w * 0.9);
    const bx = op.x1 + ux * shaftEnd;
    const by = op.y1 + uy * shaftEnd;
    ctx.beginPath();
    ctx.moveTo(op.x1 + px * startHalf, op.y1 + py * startHalf);
    ctx.lineTo(bx + px * endHalf, by + py * endHalf);
    ctx.lineTo(bx + px * headHalf, by + py * headHalf);
    ctx.lineTo(op.x2, op.y2);
    ctx.lineTo(bx - px * headHalf, by - py * headHalf);
    ctx.lineTo(bx - px * endHalf, by - py * endHalf);
    ctx.lineTo(op.x1 - px * startHalf, op.y1 - py * startHalf);
    ctx.closePath();
    ctx.fill();
  }

  function drawPen(ctx, op) {
    if (!op.points.length) return;
    ctx.beginPath();
    ctx.moveTo(op.points[0].x, op.points[0].y);
    for (let i = 1; i < op.points.length; i++) {
      ctx.lineTo(op.points[i].x, op.points[i].y);
    }
    ctx.stroke();
  }

  function drawText(ctx, op) {
    if (!op.text) return;
    setTextFont(ctx, op.size);
    ctx.textBaseline = "top";
    ctx.fillText(op.text, op.x, op.y);
  }

  function mosaicBlock(width) {
    return Math.max(5, Math.round((width || 4) * 1.5));
  }

  function drawMosaic(ctx, op) {
    if (!op.points.length || !img.naturalWidth) return;
    const block = mosaicBlock(op.width);
    const sw = state.width;
    const sh = state.height;
    let last = null;
    for (const p of op.points) {
      if (last && Math.hypot(p.x - last.x, p.y - last.y) < block / 2) continue;
      last = p;
      const sx = Math.max(0, Math.min(sw - block, Math.round(p.x - block / 2)));
      const sy = Math.max(0, Math.min(sh - block, Math.round(p.y - block / 2)));
      const bw = Math.min(block, sw - sx);
      const bh = Math.min(block, sh - sy);
      if (bw < 2 || bh < 2) continue;
      // 更小的格子 + 双线性缩放，让相邻色块过渡柔和。
      const tw = Math.max(1, Math.round(bw / 4));
      const th = Math.max(1, Math.round(bh / 4));
      mosaicScratch.width = tw;
      mosaicScratch.height = th;
      const tiny = mosaicScratch.getContext("2d");
      tiny.imageSmoothingEnabled = true;
      tiny.drawImage(img, sx, sy, bw, bh, 0, 0, tw, th);
      ctx.save();
      ctx.imageSmoothingEnabled = true;
      // 轻微外扩重叠，相邻格子边缘互相融合。
      ctx.drawImage(mosaicScratch, 0, 0, tw, th, sx - 1, sy - 1, bw + 2, bh + 2);
      ctx.restore();
    }
  }

  function dashedRect(ctx, b) {
    ctx.save();
    ctx.setLineDash([6 * dpr, 4 * dpr]);
    ctx.lineWidth = Math.max(1, dpr);
    ctx.strokeStyle = accent;
    ctx.strokeRect(b.x, b.y, b.w, b.h);
    ctx.restore();
  }

  function dashedLine(ctx, x1, y1, x2, y2) {
    ctx.save();
    ctx.setLineDash([6 * dpr, 4 * dpr]);
    ctx.lineWidth = Math.max(1, dpr);
    ctx.strokeStyle = accent;
    ctx.beginPath();
    ctx.moveTo(x1, y1);
    ctx.lineTo(x2, y2);
    ctx.stroke();
    ctx.restore();
  }

  function drawHandle(ctx, h) {
    // 所有控制点统一圆形，半径约为旧版方形的 65%。
    const r = 3.6 * dpr;
    ctx.save();
    ctx.setLineDash([]);
    ctx.lineWidth = Math.max(1, 1.3 * dpr);
    ctx.strokeStyle = accent;
    ctx.fillStyle = "#ffffff";
    ctx.beginPath();
    ctx.arc(h.x, h.y, r, 0, Math.PI * 2);
    ctx.fill();
    ctx.stroke();
    ctx.restore();
  }

  function drawSelection(ctx) {
    const op = findOp(selectedId);
    if (!op) return;
    if (op.tool === "arrow") {
      dashedLine(ctx, op.x1, op.y1, op.x2, op.y2);
      for (const h of handlesOf(op)) drawHandle(ctx, h);
      return;
    }
    const b = opBounds(op);
    dashedRect(ctx, b);
    for (const h of handlesOf(op)) drawHandle(ctx, h);
  }

  function drawDraftHelpers(ctx) {
    if (!draft) return;
    if (draft.tool === "arrow") {
      dashedLine(ctx, draft.x1, draft.y1, draft.x2, draft.y2);
      drawHandle(ctx, { x: draft.x1, y: draft.y1 });
      drawHandle(ctx, { x: draft.x2, y: draft.y2 });
      return;
    }
    if (draft.tool === "rect" || draft.tool === "ellipse") {
      const b = { x: draft.x, y: draft.y, w: draft.w, h: draft.h };
      dashedRect(ctx, b);
      const fake = { ...draft, ...b };
      for (const h of handlesOf(fake)) drawHandle(ctx, h);
    }
  }

  // ── 文字输入 ─────────────────────────────────────────────────────────
  function textScale() {
    return annot && annot.width ? (annot.clientWidth || 0) / annot.width : 1;
  }

  function textInputWidth(value, size, scale) {
    const m = measureText(value || " ", size);
    return Math.max(32, Math.ceil(m.w * scale) + 24);
  }

  function fitTextInput() {
    if (!textInput) return;
    const scale = textScale();
    const { el: input, size } = textInput;
    input.style.width = textInputWidth(input.value, size, scale) + "px";
    input.style.height = Math.max(18, Math.ceil(size * 1.3 * scale)) + "px";
    input.style.fontSize = size * scale + "px";
  }

  function createTextInput(op) {
    const scale = textScale();
    const input = document.createElement("input");
    input.type = "text";
    input.className = "pv-text-input";
    input.maxLength = 500;
    input.value = op.text || "";
    input.style.left = op.x * scale + "px";
    input.style.top = op.y * scale + "px";
    input.style.color = op.color;
    input.style.fontFamily = textFont;
    input.style.fontWeight = "600";
    input.style.lineHeight = "1.2";
    annot.parentElement.appendChild(input);
    textInput = { el: input, x: op.x, y: op.y, size: op.size, editOp: op.editOp || null };
    fitTextInput();
    setTimeout(() => {
      input.focus();
      input.select();
    }, 0);
    input.addEventListener("input", fitTextInput);
    input.addEventListener("keydown", (ev) => {
      if (ev.isComposing) return;
      if (ev.key === "Enter") {
        ev.preventDefault();
        commitText();
      } else if (ev.key === "Escape") {
        ev.preventDefault();
        cancelTextInput();
      }
      ev.stopPropagation();
    });
    input.addEventListener("blur", () => commitText());
    input.addEventListener("pointerdown", (ev) => ev.stopPropagation());
    input.addEventListener("mousedown", (ev) => ev.stopPropagation());
  }

  function openText(p) {
    commitText();
    if (!annot || !annot.parentElement) return;
    selectedId = null;
    const size = TEXT_SIZES[settings.text.level] * dpr;
    createTextInput({
      x: p.x,
      y: p.y,
      size,
      color,
      text: "",
      editOp: null,
    });
    renderAnnot();
  }

  function beginTextEdit(op) {
    commitText();
    if (!op || op.tool !== "text") return;
    selectedId = op.id;
    editingId = op.id;
    createTextInput({
      x: op.x,
      y: op.y,
      size: op.size,
      color: op.color,
      text: op.text,
      editOp: op,
    });
    renderAnnot();
  }

  function commitText() {
    if (!textInput) return;
    const { el: input, x, y, size, editOp } = textInput;
    const text = input.value.trim();
    textInput = null;
    input.remove();
    editingId = null;
    let changed = false;
    if (editOp) {
      if (!text) {
        ops = ops.filter((op) => op !== editOp);
        if (selectedId === editOp.id) selectedId = null;
      } else {
        editOp.text = text;
        editOp.x = x;
        editOp.y = y;
        refreshTextMetrics(editOp);
      }
      changed = true;
    } else if (text) {
      const op = {
        id: nextId++,
        tool: "text",
        color,
        width: 0,
        x,
        y,
        size,
        text,
      };
      refreshTextMetrics(op);
      ops.push(op);
      changed = true;
    }
    if (changed) commitSnapshot();
    renderAnnot();
  }

  function cancelTextInput() {
    if (!textInput) return;
    const { el: input } = textInput;
    textInput = null;
    input.remove();
    editingId = null;
    renderAnnot();
  }

  // ── 指针交互 ─────────────────────────────────────────────────────────
  function toImage(ev) {
    const r = annot.getBoundingClientRect();
    const w = r.width || 1;
    const h = r.height || 1;
    return {
      x: Math.max(0, Math.min(annot.width, ((ev.clientX - r.left) / w) * annot.width)),
      y: Math.max(0, Math.min(annot.height, ((ev.clientY - r.top) / h) * annot.height)),
    };
  }

  function startDraft(p, pointerId) {
    if (!tool) return;
    selectedId = null;
    closePop();
    dragId = pointerId;
    try {
      annot.setPointerCapture(pointerId);
    } catch (_) {
      /* 指针捕获失败也能继续画 */
    }
    const cfg = settings[tool] || {};
    const width = WIDTHS[cfg.level || 0] * dpr;
    if (tool === "pen" || tool === "mosaic") {
      draft = { tool, color, width, points: [p] };
    } else if (tool === "arrow") {
      draft = { tool, color, width, x1: p.x, y1: p.y, x2: p.x, y2: p.y };
    } else {
      draft = {
        tool,
        color,
        width,
        fill: !!cfg.fill,
        x0: p.x,
        y0: p.y,
        x: p.x,
        y: p.y,
        w: 0,
        h: 0,
      };
    }
    scheduleRender();
  }

  function moveOp(op, orig, dx, dy) {
    if (op.tool === "rect" || op.tool === "ellipse" || op.tool === "text") {
      op.x = orig.x + dx;
      op.y = orig.y + dy;
    } else if (op.tool === "arrow") {
      op.x1 = orig.x1 + dx;
      op.y1 = orig.y1 + dy;
      op.x2 = orig.x2 + dx;
      op.y2 = orig.y2 + dy;
    } else if (orig.points) {
      op.points = orig.points.map((q) => ({ x: q.x + dx, y: q.y + dy }));
    }
  }

  function resizeOp(op, orig, handle, p) {
    if (op.tool === "arrow") {
      if (handle === "p1") {
        op.x1 = p.x;
        op.y1 = p.y;
      } else {
        op.x2 = p.x;
        op.y2 = p.y;
      }
      return;
    }
    let x = orig.x;
    let y = orig.y;
    let w = orig.w;
    let h = orig.h;
    const min = 4;
    if (handle.includes("n")) {
      y = p.y;
      h = orig.y + orig.h - p.y;
    }
    if (handle.includes("s")) h = p.y - orig.y;
    if (handle.includes("w")) {
      x = p.x;
      w = orig.x + orig.w - p.x;
    }
    if (handle.includes("e")) w = p.x - orig.x;
    if (w < min) {
      if (handle.includes("w")) x = orig.x + orig.w - min;
      w = min;
    }
    if (h < min) {
      if (handle.includes("n")) y = orig.y + orig.h - min;
      h = min;
    }
    op.x = x;
    op.y = y;
    op.w = w;
    op.h = h;
  }

  function updateCursor(ev) {
    if (!annot || dragId !== null) return;
    const p = toImage(ev);
    const sel = findOp(selectedId);
    const h = sel && handleAt(sel, p);
    if (h) {
      annot.style.cursor = HANDLE_CURSORS[h.key] || "crosshair";
      hideBrush();
      return;
    }
    const hit = topOpAt(p);
    if (hit) {
      annot.style.cursor = "move";
      hideBrush();
      return;
    }
    if (tool === "mosaic") {
      annot.style.cursor = "none";
      showBrush(ev, p);
      return;
    }
    hideBrush();
    annot.style.cursor = tool === "text" ? "text" : tool ? "crosshair" : "default";
  }

  function showBrush(ev, p) {
    if (!imageEditable() || tool !== "mosaic") {
      hideBrush();
      return;
    }
    const level = settings.mosaic.level;
    const width = WIDTHS[level] * dpr;
    const block = mosaicBlock(width);
    const scale = annot.width ? (annot.clientWidth || 0) / annot.width : 1;
    const css = Math.max(2, Math.round(block * (scale || 1)));
    brush.style.width = css + "px";
    brush.style.height = css + "px";
    brush.style.left = ev.clientX + "px";
    brush.style.top = ev.clientY + "px";
    brush.classList.add("on");
  }

  function hideBrush() {
    brush.classList.remove("on");
  }

  if (annot) {
    annot.addEventListener("pointerdown", (ev) => {
      if (!editable() || ev.button !== 0) return;
      ev.preventDefault();
      if (textInput) commitText();
      const p = toImage(ev);
      closePop();

      const sel = findOp(selectedId);
      const handle = sel && handleAt(sel, p);
      if (handle) {
        dragId = ev.pointerId;
        try {
          annot.setPointerCapture(ev.pointerId);
        } catch (_) {
          /* ignore */
        }
        interaction = {
          mode: "resize",
          op: sel,
          orig: structuredClone(sel),
          handle: handle.key,
          moved: false,
        };
        renderAnnot();
        return;
      }

      const hit = topOpAt(p);
      if (hit) {
        selectedId = hit.id;
        dragId = ev.pointerId;
        try {
          annot.setPointerCapture(ev.pointerId);
        } catch (_) {
          /* ignore */
        }
        interaction = {
          mode: "move",
          op: hit,
          orig: structuredClone(hit),
          start: p,
          moved: false,
        };
        renderAnnot();
        return;
      }

      selectedId = null;
      renderAnnot();
      if (!tool) return;

      if (tool === "text") {
        openText(p);
        return;
      }
      startDraft(p, ev.pointerId);
    });

    annot.addEventListener("pointermove", (ev) => {
      if (dragId === ev.pointerId) {
        const p = toImage(ev);
        if (interaction && interaction.mode === "move") {
          const dx = p.x - interaction.start.x;
          const dy = p.y - interaction.start.y;
          if (Math.hypot(dx, dy) > 1) interaction.moved = true;
          moveOp(interaction.op, interaction.orig, dx, dy);
          scheduleRender();
          return;
        }
        if (interaction && interaction.mode === "resize") {
          interaction.moved = true;
          resizeOp(interaction.op, interaction.orig, interaction.handle, p);
          scheduleRender();
          return;
        }
        if (!draft) return;
        if (draft.tool === "pen" || draft.tool === "mosaic") {
          const last = draft.points[draft.points.length - 1];
          if (Math.hypot(p.x - last.x, p.y - last.y) >= 1.5) {
            draft.points.push(p);
          }
          if (draft.tool === "mosaic") showBrush(ev, p);
        } else if (draft.tool === "arrow") {
          draft.x2 = p.x;
          draft.y2 = p.y;
        } else {
          let dx = p.x - draft.x0;
          let dy = p.y - draft.y0;
          if (ev.shiftKey && (draft.tool === "rect" || draft.tool === "ellipse")) {
            const size = Math.max(Math.abs(dx), Math.abs(dy));
            dx = dx < 0 ? -size : size;
            dy = dy < 0 ? -size : size;
          }
          draft.x = Math.min(draft.x0, draft.x0 + dx);
          draft.y = Math.min(draft.y0, draft.y0 + dy);
          draft.w = Math.abs(dx);
          draft.h = Math.abs(dy);
        }
        scheduleRender();
        return;
      }
      updateCursor(ev);
    });

    const endDrag = (ev) => {
      if (dragId !== ev.pointerId) return;
      dragId = null;
      try {
        annot.releasePointerCapture(ev.pointerId);
      } catch (_) {
        /* 已释放 */
      }
      if (interaction) {
        const moved = interaction.moved;
        interaction = null;
        if (moved) commitSnapshot();
        updateCursor(ev);
        return;
      }
      finishDraft();
    };
    annot.addEventListener("pointerup", endDrag);
    annot.addEventListener("pointercancel", endDrag);
    annot.addEventListener("pointerleave", hideBrush);

    annot.addEventListener("dblclick", (ev) => {
      if (!editable()) return;
      const p = toImage(ev);
      const op = topOpAt(p);
      if (op && op.tool === "text") {
        ev.preventDefault();
        beginTextEdit(op);
      }
    });
  }

  function finishDraft() {
    const op = draft;
    draft = null;
    if (!op) return;
    if (op.tool === "pen" || op.tool === "mosaic") {
      if (op.points.length < 2) {
        renderAnnot();
        return;
      }
    } else if (op.tool === "arrow") {
      if (Math.hypot(op.x2 - op.x1, op.y2 - op.y1) < 3) {
        renderAnnot();
        return;
      }
    } else if (op.w < 2 || op.h < 2) {
      renderAnnot();
      return;
    }
    op.id = nextId++;
    ops.push(op);
    selectedId = null;
    commitSnapshot();
    renderAnnot();
  }

  // ── 交付：有标注时先合成一张 PNG 再走既有动作 ────────────────────────
  function composeEditedBlob() {
    return new Promise((resolve, reject) => {
      const w = state.width;
      const h = state.height;
      const outH = h - head - tail;
      if (!w || !h || outH <= 0) {
        reject(new Error("裁切范围不合法"));
        return;
      }
      const full = document.createElement("canvas");
      full.width = w;
      full.height = h;
      const ctx = full.getContext("2d");
      ctx.drawImage(img, 0, 0, w, h);
      for (const op of ops) drawOp(ctx, op);

      let out = full;
      if (head || tail) {
        out = document.createElement("canvas");
        out.width = w;
        out.height = outH;
        out.getContext("2d").drawImage(full, 0, head, w, outH, 0, 0, w, outH);
      }
      out.toBlob((blob) => {
        if (blob) resolve(blob);
        else reject(new Error("合成编辑结果失败"));
      }, "image/png");
    });
  }

  async function act(kind) {
    if (busy) return;
    if (!state.url) {
      msg("没有可预览的图片");
      return;
    }
    commitText();
    setBusy(true);
    try {
      if (ops.length) {
        if (!imageEditable()) {
          msg("图片过大，暂不支持标注");
          return;
        }
        msg("正在应用标注…");
        let blob;
        try {
          blob = await composeEditedBlob();
        } catch (e) {
          msg(String(e));
          return;
        }
        const r = await invoke("preview_commit_edited", await blob.arrayBuffer());
        if (!r || !r.ok) {
          msg((r && r.message) || "应用标注失败");
          return;
        }
        applyState(r.state);
        head = 0;
        tail = 0;
        headEl.value = 0;
        tailEl.value = 0;
        labels();
      }

      msg(kind === "ocr" ? "识别中…" : "处理中…");
      const r = await invoke("preview_action", { head, tail, action: kind });
      if (!r || !r.ok) {
        msg((r && r.message) || "操作失败");
        return;
      }
      msg(r.message || "完成");
      if (kind === "ocr") {
        el("pv-ocr-box").hidden = false;
        el("pv-ocr-text").textContent = r.text || "";
        el("pv-translation").hidden = true;
        el("pv-translation-text").textContent = "";
      }
    } catch (e) {
      msg(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function translateOcr() {
    const text = el("pv-ocr-text").textContent;
    if (!text.trim()) {
      msg("没有可翻译的文字");
      return;
    }
    if (busy) return;
    setBusy(true);
    msg("翻译中…");
    try {
      const r = await invoke("translate_text_only", { text });
      if (!r || !r.dst) {
        msg("翻译失败");
        return;
      }
      el("pv-translation").hidden = false;
      el("pv-translation-text").textContent = r.dst;
      msg("已翻译");
    } catch (e) {
      msg(String(e));
    } finally {
      setBusy(false);
    }
  }

  invoke("preview_state")
    .then(applyState)
    .catch(() => {})
    .finally(() => window.TitleBar?.reveal?.());
  window.__TAURI__.event.listen("preview:state", (e) => applyState(e.payload || {}));

  headEl.addEventListener("input", clamp);
  tailEl.addEventListener("input", clamp);
  window.addEventListener("resize", () => {
    dpr = window.devicePixelRatio || 1;
    applyZoom();
    updateBands();
    syncAnnotCss();
    positionPop();
  });

  el("pv-save").addEventListener("click", () => act("save_as"));
  el("pv-copy").addEventListener("click", () => act("clipboard"));
  el("pv-pin").addEventListener("click", () => act("pin"));
  el("pv-ocr").addEventListener("click", () => act("ocr"));
  el("pv-translate").addEventListener("click", translateOcr);
  el("pv-zoom-out").addEventListener("click", () => setZoom(zoom / 1.25));
  el("pv-zoom-in").addEventListener("click", () => setZoom(zoom * 1.25));

  if (editBar) {
    for (const btn of editBar.querySelectorAll(".pv-tool")) {
      btn.addEventListener("click", () => {
        const next = btn.dataset.tool || "";
        setTool(next);
        if (next && POP_TOOLS.has(next)) openPop(btn);
        else closePop();
      });
    }
    const colors = el("pv-colors");
    if (colors) {
      colors.addEventListener("click", (ev) => {
        const btn = ev.target.closest(".pv-color");
        if (!btn || btn.disabled) return;
        color = btn.dataset.color || color;
        for (const node of colors.querySelectorAll(".pv-color")) {
          node.classList.toggle("on", node === btn);
        }
      });
    }
    editBar.addEventListener("scroll", closePop);
    el("pv-undo").addEventListener("click", undoAnnot);
    el("pv-redo").addEventListener("click", redoAnnot);
    el("pv-clear").addEventListener("click", clearAnnot);
  }

  document.addEventListener(
    "pointerdown",
    (ev) => {
      if (!popOpen || !pop) return;
      if (pop.contains(ev.target)) return;
      if (editBar && editBar.contains(ev.target)) return;
      closePop();
    },
    true
  );

  el("pv-zoom-out").innerHTML = Icons.svg("minus", 16);
  el("pv-zoom-in").innerHTML = Icons.svg("plus", 16);
  el("pv-translate").innerHTML = Icons.raw("translate", 20);
  el("pv-copy-text").innerHTML = Icons.svg("copy", 20);

  el("pv-copy-text").addEventListener("click", () => {
    const text = el("pv-ocr-text").textContent;
    if (text) invoke("copy_text", { text }).catch(() => {});
  });

  const close = () => invoke("preview_close").catch(() => {});
  window.addEventListener("keydown", (e) => {
    if (textInput) return;
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key.toLowerCase() === "z") {
      e.preventDefault();
      if (e.shiftKey) redoAnnot();
      else undoAnnot();
      return;
    }
    if (mod && e.key.toLowerCase() === "y") {
      e.preventDefault();
      redoAnnot();
      return;
    }
    if (e.key === "Escape") {
      if (popOpen) {
        closePop();
        return;
      }
      if (selectedId) {
        selectedId = null;
        renderAnnot();
        return;
      }
      if (tool) {
        setTool("");
        return;
      }
      close();
    }
  });
})();
