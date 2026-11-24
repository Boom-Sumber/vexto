// 共享标注编辑器（会话 95）：预览窗与截图选区工具条共用。
// 只负责画布上的图元、选中/移动/缩放、文字、马赛克与合成 PNG；
// 交付（存盘 / 剪贴板 / 钉屏 / 预览）由宿主页面决定。
import { t } from "../i18n/index.js";

const AnnotEditor = (function () {
  const WIDTHS = [2, 4, 8];
  const TEXT_SIZES = [14, 20, 28];
  const COLORS = ["#f0566d", "#f59e0b", "#22c55e", "#3b82f6", "#ffffff", "#111111"];
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
  const TEXT_FONT = '"Microsoft YaHei", "Segoe UI", sans-serif';

  function create(opts) {
    const canvas = opts.canvas;
    if (!canvas) throw new Error("AnnotEditor 需要 canvas");
    const ctx = canvas.getContext("2d");
    const onChange = typeof opts.onChange === "function" ? opts.onChange : () => {};
    const getScale =
      typeof opts.getScale === "function"
        ? opts.getScale
        : () => (canvas.width ? (canvas.clientWidth || 0) / canvas.width : 1);

    let dpr = opts.dpr || window.devicePixelRatio || 1;
    let enabled = false;
    let base = null;
    let baseW = 0;
    let baseH = 0;
    let tool = "";
    let color = opts.initialColor || COLORS[0];
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
    let popOpen = false;
    let popAnchor = null;
    const mosaicScratch = document.createElement("canvas");
    const measureCanvas = document.createElement("canvas");
    const measureCtx = measureCanvas.getContext("2d");
    const settings = {
      rect: { level: 1, fill: false },
      ellipse: { level: 1, fill: false },
      arrow: { level: 1 },
      pen: { level: 1 },
      mosaic: { level: 1 },
      text: { level: 1 },
    };

    const pop = document.createElement("div");
    pop.className = "ae-pop";
    pop.hidden = true;
    pop.innerHTML = '<span class="ae-pop-arrow"></span><div class="ae-pop-body"></div>';
    document.body.appendChild(pop);
    const popBody = pop.querySelector(".ae-pop-body");

    const brush = document.createElement("div");
    brush.className = "ae-brush";
    document.body.appendChild(brush);

    function accent() {
      return (
        getComputedStyle(document.body).getPropertyValue("--accent").trim() ||
        "#11a780"
      );
    }

    function setTextFont(c, size) {
      c.font = "600 " + size + "px " + TEXT_FONT;
    }

    function measureText(text, size) {
      if (!measureCtx) return { w: size, h: size * 1.2 };
      setTextFont(measureCtx, size);
      return {
        w: Math.ceil(measureCtx.measureText(text).width) + 2,
        h: Math.ceil(size * 1.2),
      };
    }

    function refreshTextMetrics(op) {
      const m = measureText(op.text, op.size);
      op.w = m.w;
      op.h = m.h;
    }

    function notify() {
      onChange(ops.length > 0);
    }

    function findOp(id) {
      return ops.find((op) => op.id === id) || null;
    }

    function commitSnapshot() {
      history = history.slice(0, hp + 1);
      history.push(structuredClone(ops));
      hp = history.length - 1;
      notify();
    }

    function resetState() {
      cancelTextInput();
      closePop();
      hideBrush();
      ops = [];
      history = [[]];
      hp = 0;
      nextId = 1;
      selectedId = null;
      draft = null;
      dragId = null;
      interaction = null;
      editingId = null;
      notify();
    }

    function syncCursor() {
      if (!canvas) return;
      if (!enabled) {
        canvas.style.cursor = "default";
        return;
      }
      if (tool === "mosaic") canvas.style.cursor = "none";
      else if (tool === "text") canvas.style.cursor = "text";
      else if (tool) canvas.style.cursor = "crosshair";
      else canvas.style.cursor = "default";
    }

    function setSource(src, width, height) {
      base = src;
      baseW = Math.max(0, Math.round(width || 0));
      baseH = Math.max(0, Math.round(height || 0));
      canvas.width = Math.max(1, baseW);
      canvas.height = Math.max(1, baseH);
      resetState();
      render();
    }

    function setEnabled(flag) {
      enabled = !!flag;
      canvas.hidden = !enabled;
      canvas.style.pointerEvents = enabled ? "auto" : "none";
      if (!enabled) {
        closePop();
        cancelTextInput();
        draft = null;
        dragId = null;
        interaction = null;
        hideBrush();
      }
      syncCursor();
      render();
    }

    function setTool(next) {
      if (dragId !== null) {
        draft = null;
        dragId = null;
        interaction = null;
      }
      tool = enabled ? next || "" : "";
      canvas.classList.toggle("drawing", !!tool);
      canvas.classList.toggle("textmode", tool === "text");
      if (tool !== "mosaic") hideBrush();
      if (!tool) closePop();
      syncCursor();
    }

    function setColor(next) {
      if (next) color = next;
    }

    function setDpr(next) {
      if (next > 0) dpr = next;
    }

    function hasAnnotations() {
      return ops.length > 0;
    }

    function closePop() {
      pop.hidden = true;
      popOpen = false;
      popAnchor = null;
    }

    function openPopup(anchor) {
      if (!enabled || !POP_TOOLS.has(tool)) return;
      popAnchor = anchor;
      popOpen = true;
      pop.hidden = false;
      renderPop();
      positionPop();
    }

    function positionPop() {
      if (!popOpen || !popAnchor) return;
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
      b.title = [t("annot.thin"), t("annot.medium"), t("annot.thick")][level];
      b.style.setProperty("--dot", [5, 8, 11][level] + "px");
      const cfg = settings[tool];
      const solidShape = (tool === "rect" || tool === "ellipse") && cfg.fill;
      b.classList.toggle("on", !solidShape && cfg.level === level);
      b.disabled = solidShape;
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
      b.title = t(cfg.fill ? "annot.fill" : "annot.hollow");
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
      b.textContent = [t("annot.sizeSmall"), t("annot.sizeMedium"), t("annot.sizeLarge")][level];
      b.addEventListener("click", () => {
        settings.text.level = level;
        renderPop();
      });
      return b;
    }

    function makeColor(hex) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "ae-color" + (color === hex ? " on" : "");
      b.title = t("annot.color", { hex });
      b.style.setProperty("--sw", hex);
      b.addEventListener("click", () => {
        color = hex;
        renderPop();
      });
      return b;
    }

    function sep() {
      const s = document.createElement("span");
      s.className = "ae-pop-sep";
      return s;
    }

    function renderPop() {
      if (!popBody || !POP_TOOLS.has(tool)) return;
      popBody.textContent = "";
      if (tool === "text") {
        for (let i = 0; i < 3; i++) popBody.appendChild(makeTextSize(i));
      } else {
        for (let i = 0; i < 3; i++) popBody.appendChild(makeDot(i));
        if (tool === "rect" || tool === "ellipse") {
          popBody.appendChild(sep());
          popBody.appendChild(makeFill());
        }
      }
      // 马赛克不需要颜色：像素块颜色来自底图。
      if (tool === "mosaic") return;
      popBody.appendChild(sep());
      const colors = document.createElement("div");
      colors.className = "ae-colors";
      for (const hex of COLORS) colors.appendChild(makeColor(hex));
      popBody.appendChild(colors);
    }

    // ── 几何与命中 ──────────────────────────────────────────────────────
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
      if (op.tool === "mosaic") return false;
      if (op.tool === "text") return pointInBounds(p, opBounds(op));
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
          distToSegment(p, { x: op.x1, y: op.y1 }, { x: op.x2, y: op.y2 }) <=
          Math.max(10, (op.width || 2) * 2.5)
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
      const scale = getScale() || 1;
      const tol = 10 / scale;
      for (const h of handlesOf(op)) {
        if (Math.hypot(p.x - h.x, p.y - h.y) <= tol) return h;
      }
      return null;
    }

    // ── 绘制 ────────────────────────────────────────────────────────────
    function render() {
      if (!ctx || canvas.hidden || !baseW || !baseH) return;
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      if (base) ctx.drawImage(base, 0, 0, canvas.width, canvas.height);
      for (const op of ops) {
        if (op.id === editingId) continue;
        drawOp(ctx, op);
      }
      if (draft) drawOp(ctx, draft);
      if (selectedId) drawSelection(ctx);
      if (draft) drawDraftHelpers(ctx);
    }

    function scheduleRender() {
      if (renderQueued) return;
      renderQueued = true;
      requestAnimationFrame(() => {
        renderQueued = false;
        render();
      });
    }

    function drawOp(c, op) {
      c.save();
      c.strokeStyle = op.color;
      c.fillStyle = op.color;
      c.lineWidth = Math.max(1, op.width || 2);
      c.lineCap = "round";
      c.lineJoin = "round";
      switch (op.tool) {
        case "rect":
          if (op.w > 0 && op.h > 0) {
            if (op.fill) c.fillRect(op.x, op.y, op.w, op.h);
            else c.strokeRect(op.x, op.y, op.w, op.h);
          }
          break;
        case "ellipse":
          if (op.w > 0 && op.h > 0) {
            c.beginPath();
            c.ellipse(
              op.x + op.w / 2,
              op.y + op.h / 2,
              Math.max(op.w / 2, 0.5),
              Math.max(op.h / 2, 0.5),
              0,
              0,
              Math.PI * 2
            );
            if (op.fill) c.fill();
            else c.stroke();
          }
          break;
        case "arrow":
          drawArrow(c, op);
          break;
        case "pen":
          drawPen(c, op);
          break;
        case "mosaic":
          drawMosaic(c, op);
          break;
        case "text":
          drawText(c, op);
          break;
        default:
          break;
      }
      c.restore();
    }

    function drawArrow(c, op) {
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
      const headHalf = headLen * 0.4;
      const shaftEnd = Math.max(0, len - headLen);
      const startHalf = Math.max(0.5, dpr);
      const endHalf = Math.max(0.8, w * 0.9);
      const bx = op.x1 + ux * shaftEnd;
      const by = op.y1 + uy * shaftEnd;
      c.beginPath();
      c.moveTo(op.x1 + px * startHalf, op.y1 + py * startHalf);
      c.lineTo(bx + px * endHalf, by + py * endHalf);
      c.lineTo(bx + px * headHalf, by + py * headHalf);
      c.lineTo(op.x2, op.y2);
      c.lineTo(bx - px * headHalf, by - py * headHalf);
      c.lineTo(bx - px * endHalf, by - py * endHalf);
      c.lineTo(op.x1 - px * startHalf, op.y1 - py * startHalf);
      c.closePath();
      c.fill();
    }

    function drawPen(c, op) {
      if (!op.points.length) return;
      c.beginPath();
      c.moveTo(op.points[0].x, op.points[0].y);
      for (let i = 1; i < op.points.length; i++) c.lineTo(op.points[i].x, op.points[i].y);
      c.stroke();
    }

    function drawText(c, op) {
      if (!op.text) return;
      setTextFont(c, op.size);
      c.textBaseline = "top";
      c.fillText(op.text, op.x, op.y);
    }

    function mosaicBlock(width) {
      return Math.max(5, Math.round((width || 4) * 1.5));
    }

    function drawMosaic(c, op) {
      if (!op.points.length || !base) return;
      const block = mosaicBlock(op.width);
      const sw = baseW;
      const sh = baseH;
      let last = null;
      for (const p of op.points) {
        if (last && Math.hypot(p.x - last.x, p.y - last.y) < block / 2) continue;
        last = p;
        const sx = Math.max(0, Math.min(sw - block, Math.round(p.x - block / 2)));
        const sy = Math.max(0, Math.min(sh - block, Math.round(p.y - block / 2)));
        const bw = Math.min(block, sw - sx);
        const bh = Math.min(block, sh - sy);
        if (bw < 2 || bh < 2) continue;
        const tw = Math.max(1, Math.round(bw / 4));
        const th = Math.max(1, Math.round(bh / 4));
        mosaicScratch.width = tw;
        mosaicScratch.height = th;
        const tiny = mosaicScratch.getContext("2d");
        tiny.imageSmoothingEnabled = true;
        tiny.drawImage(base, sx, sy, bw, bh, 0, 0, tw, th);
        c.save();
        c.imageSmoothingEnabled = true;
        c.drawImage(mosaicScratch, 0, 0, tw, th, sx - 1, sy - 1, bw + 2, bh + 2);
        c.restore();
      }
    }

    function dashedRect(c, b) {
      c.save();
      c.setLineDash([6 * dpr, 4 * dpr]);
      c.lineWidth = Math.max(1, dpr);
      c.strokeStyle = accent();
      c.strokeRect(b.x, b.y, b.w, b.h);
      c.restore();
    }

    function dashedLine(c, x1, y1, x2, y2) {
      c.save();
      c.setLineDash([6 * dpr, 4 * dpr]);
      c.lineWidth = Math.max(1, dpr);
      c.strokeStyle = accent();
      c.beginPath();
      c.moveTo(x1, y1);
      c.lineTo(x2, y2);
      c.stroke();
      c.restore();
    }

    function drawHandle(c, h) {
      const r = 3.6 * dpr;
      c.save();
      c.setLineDash([]);
      c.lineWidth = Math.max(1, 1.3 * dpr);
      c.strokeStyle = accent();
      c.fillStyle = "#ffffff";
      c.beginPath();
      c.arc(h.x, h.y, r, 0, Math.PI * 2);
      c.fill();
      c.stroke();
      c.restore();
    }

    function drawSelection(c) {
      const op = findOp(selectedId);
      if (!op) return;
      if (op.tool === "arrow") {
        dashedLine(c, op.x1, op.y1, op.x2, op.y2);
        for (const h of handlesOf(op)) drawHandle(c, h);
        return;
      }
      const b = opBounds(op);
      dashedRect(c, b);
      for (const h of handlesOf(op)) drawHandle(c, h);
    }

    function drawDraftHelpers(c) {
      if (!draft) return;
      if (draft.tool === "arrow") {
        dashedLine(c, draft.x1, draft.y1, draft.x2, draft.y2);
        drawHandle(c, { x: draft.x1, y: draft.y1 });
        drawHandle(c, { x: draft.x2, y: draft.y2 });
        return;
      }
      if (draft.tool === "rect" || draft.tool === "ellipse") {
        const b = { x: draft.x, y: draft.y, w: draft.w, h: draft.h };
        dashedRect(c, b);
        const fake = { ...draft, ...b };
        for (const h of handlesOf(fake)) drawHandle(c, h);
      }
    }

    // ── 文字输入 ────────────────────────────────────────────────────────
    function textInputWidth(value, size, scale) {
      const m = measureText(value || " ", size);
      return Math.max(32, Math.ceil(m.w * scale) + 24);
    }

    function fitTextInput() {
      if (!textInput) return;
      const scale = getScale() || 1;
      const { el, size } = textInput;
      el.style.width = textInputWidth(el.value, size, scale) + "px";
      el.style.height = Math.max(18, Math.ceil(size * 1.3 * scale)) + "px";
      el.style.fontSize = size * scale + "px";
    }

    function createTextInput(cfg) {
      const scale = getScale() || 1;
      const parent = canvas.parentElement;
      const cr = canvas.getBoundingClientRect();
      const pr = parent.getBoundingClientRect();
      const input = document.createElement("input");
      input.type = "text";
      input.className = "ae-text-input";
      input.maxLength = 500;
      input.value = cfg.text || "";
      input.style.left = cr.left - pr.left + cfg.x * scale + "px";
      input.style.top = cr.top - pr.top + cfg.y * scale + "px";
      input.style.color = cfg.color;
      input.style.fontFamily = TEXT_FONT;
      input.style.fontWeight = "600";
      input.style.lineHeight = "1.2";
      parent.appendChild(input);
      textInput = {
        el: input,
        x: cfg.x,
        y: cfg.y,
        size: cfg.size,
        color: cfg.color,
        editOp: cfg.editOp || null,
      };
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
      selectedId = null;
      createTextInput({
        x: p.x,
        y: p.y,
        size: TEXT_SIZES[settings.text.level] * dpr,
        color,
        text: "",
        editOp: null,
      });
      render();
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
      render();
    }

    function commitText() {
      if (!textInput) return;
      const { el, x, y, size, color: opColor, editOp } = textInput;
      const text = el.value.trim();
      textInput = null;
      el.remove();
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
          color: opColor,
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
      render();
    }

    function cancelTextInput() {
      if (!textInput) return;
      const { el } = textInput;
      textInput = null;
      el.remove();
      editingId = null;
      render();
    }

    // ── 指针交互 ────────────────────────────────────────────────────────
    function toImage(ev) {
      const r = canvas.getBoundingClientRect();
      const w = r.width || 1;
      const h = r.height || 1;
      return {
        x: Math.max(0, Math.min(canvas.width, ((ev.clientX - r.left) / w) * canvas.width)),
        y: Math.max(0, Math.min(canvas.height, ((ev.clientY - r.top) / h) * canvas.height)),
      };
    }

    function startDraft(p, pointerId) {
      if (!tool) return;
      selectedId = null;
      closePop();
      dragId = pointerId;
      try {
        canvas.setPointerCapture(pointerId);
      } catch (_) {
        /* ignore */
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
      if (!enabled || dragId !== null) return;
      const p = toImage(ev);
      const sel = findOp(selectedId);
      const h = sel && handleAt(sel, p);
      if (h) {
        canvas.style.cursor = HANDLE_CURSORS[h.key] || "crosshair";
        hideBrush();
        return;
      }
      const hit = topOpAt(p);
      if (hit) {
        canvas.style.cursor = "move";
        hideBrush();
        return;
      }
      if (tool === "mosaic") {
        canvas.style.cursor = "none";
        showBrush(ev);
        return;
      }
      hideBrush();
      syncCursor();
    }

    function showBrush(ev) {
      if (!enabled || tool !== "mosaic") {
        hideBrush();
        return;
      }
      const level = settings.mosaic.level;
      const width = WIDTHS[level] * dpr;
      const block = mosaicBlock(width);
      const scale = getScale() || 1;
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

    canvas.addEventListener("pointerdown", (ev) => {
      if (!enabled || ev.button !== 0) return;
      ev.preventDefault();
      if (textInput) commitText();
      const p = toImage(ev);
      closePop();
      const sel = findOp(selectedId);
      const h = sel && handleAt(sel, p);
      if (h) {
        dragId = ev.pointerId;
        try {
          canvas.setPointerCapture(ev.pointerId);
        } catch (_) {
          /* ignore */
        }
        interaction = {
          mode: "resize",
          op: sel,
          orig: structuredClone(sel),
          handle: h.key,
          moved: false,
        };
        render();
        return;
      }
      const hit = topOpAt(p);
      if (hit) {
        selectedId = hit.id;
        dragId = ev.pointerId;
        try {
          canvas.setPointerCapture(ev.pointerId);
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
        render();
        return;
      }
      selectedId = null;
      render();
      if (!tool) return;
      if (tool === "text") {
        openText(p);
        return;
      }
      startDraft(p, ev.pointerId);
    });

    canvas.addEventListener("pointermove", (ev) => {
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
          if (Math.hypot(p.x - last.x, p.y - last.y) >= 1.5) draft.points.push(p);
          if (draft.tool === "mosaic") showBrush(ev);
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
        canvas.releasePointerCapture(ev.pointerId);
      } catch (_) {
        /* ignore */
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
    canvas.addEventListener("pointerup", endDrag);
    canvas.addEventListener("pointercancel", endDrag);
    canvas.addEventListener("pointerleave", hideBrush);

    canvas.addEventListener("dblclick", (ev) => {
      if (!enabled) return;
      const p = toImage(ev);
      const op = topOpAt(p);
      if (op && op.tool === "text") {
        ev.preventDefault();
        beginTextEdit(op);
      }
    });

    function finishDraft() {
      const op = draft;
      draft = null;
      if (!op) return;
      if (op.tool === "pen" || op.tool === "mosaic") {
        if (op.points.length < 2) {
          render();
          return;
        }
      } else if (op.tool === "arrow") {
        if (Math.hypot(op.x2 - op.x1, op.y2 - op.y1) < 3) {
          render();
          return;
        }
      } else if (op.w < 2 || op.h < 2) {
        render();
        return;
      }
      op.id = nextId++;
      ops.push(op);
      selectedId = null;
      commitSnapshot();
      render();
    }

    function undo() {
      if (hp <= 0) return;
      cancelTextInput();
      draft = null;
      interaction = null;
      dragId = null;
      hp -= 1;
      ops = structuredClone(history[hp]);
      if (!findOp(selectedId)) selectedId = null;
      notify();
      render();
    }

    function redo() {
      if (hp >= history.length - 1) return;
      cancelTextInput();
      draft = null;
      interaction = null;
      dragId = null;
      hp += 1;
      ops = structuredClone(history[hp]);
      if (!findOp(selectedId)) selectedId = null;
      notify();
      render();
    }

    function clear() {
      if (!ops.length) return;
      cancelTextInput();
      ops = [];
      selectedId = null;
      commitSnapshot();
      render();
    }

    function composeBlob(crop) {
      return new Promise((resolve, reject) => {
        const w = baseW;
        const h = baseH;
        if (!w || !h || !base) {
          reject(new Error(t("annot.noImage")));
          return;
        }
        const full = document.createElement("canvas");
        full.width = w;
        full.height = h;
        const fullCtx = full.getContext("2d");
        fullCtx.drawImage(base, 0, 0, w, h);
        for (const op of ops) drawOp(fullCtx, op);
        let out = full;
        const c = crop || { x: 0, y: 0, w, h };
        if (c.x || c.y || c.w !== w || c.h !== h) {
          if (c.w <= 0 || c.h <= 0) {
            reject(new Error(t("annot.invalidCrop")));
            return;
          }
          out = document.createElement("canvas");
          out.width = c.w;
          out.height = c.h;
          out.getContext("2d").drawImage(full, c.x, c.y, c.w, c.h, 0, 0, c.w, c.h);
        }
        out.toBlob((blob) => {
          if (blob) resolve(blob);
          else reject(new Error(t("annot.composeFailed")));
        }, "image/png");
      });
    }

    document.addEventListener(
      "pointerdown",
      (ev) => {
        if (!popOpen) return;
        if (pop.contains(ev.target)) return;
        if (popAnchor && popAnchor.contains(ev.target)) return;
        closePop();
      },
      true
    );
    window.addEventListener("resize", () => {
      if (popOpen) positionPop();
    });

    return {
      setSource,
      setEnabled,
      setTool,
      setColor,
      getColor: () => color,
      setDpr,
      openPopup,
      closePopup: closePop,
      render,
      hasAnnotations,
      commitPending: commitText,
      undo,
      redo,
      clear,
      composeBlob,
      get tool() {
        return tool;
      },
    };
  }

  return { create };
})();

window.AnnotEditor = AnnotEditor;
export { AnnotEditor };
