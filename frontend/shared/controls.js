// 原生控件的自绘行为（样式在 base.css）。
//
// 现有唯一成员：数字输入的上下步进。
// 系统 `::-webkit-inner-spin-button` 在深色主题下永远是白底灰三角，
// 而且无法改色；所以 CSS 把它归零，这里补一排可点的自绘按钮。
//
// 用法：
//   NumField.enhance(input)                     // 值变化时派发 input / change
//   NumField.enhance(input, (n) => commit(n))   // 由调用方接管保存
//
// 约定（docs/13-UI-DESIGN-SYSTEM.md §13.9）：
// - 宿主元素必须是 position:relative（.input-wrap / .num-row / 自定义容器）；
// - 宿主加 `.has-stepper`，输入框的右内边距由 CSS 自动让出；
// - 按钮 `tabindex="-1"`，键盘焦点始终留在输入框（Tab 顺序不变）。
import { t } from "../i18n/index.js";

const NumField = (function () {
  /// 步长：显式 step 优先，否则 1；不合法一律当 1。
  function stepOf(input) {
    const s = parseFloat(input.step);
    return Number.isFinite(s) && s > 0 ? s : 1;
  }

  function bound(input, name, fallback) {
    const raw = input[name];
    if (raw === "" || raw == null) return fallback;
    const n = Number(raw);
    return Number.isFinite(n) ? n : fallback;
  }

  function clamp(input, n) {
    const min = bound(input, "min", -Infinity);
    const max = bound(input, "max", Infinity);
    if (n < min) n = min;
    if (n > max) n = max;
    // 步进是小数时避免 0.30000000000000004 这种尾巴
    return Math.round(n * 1e6) / 1e6;
  }

  /// 当前值。空串按最小值（没有 min 则 0）算，符合「点一下先给个起点」的直觉。
  function valueOf(input) {
    const n = Number(input.value);
    if (input.value.trim() === "" || !Number.isFinite(n)) {
      return bound(input, "min", 0);
    }
    return n;
  }

  function enhance(input, onChange) {
    if (!input || input.dataset.stepper === "1") return input;
    const host = input.parentElement;
    if (!host) return input;

    input.dataset.stepper = "1";
    host.classList.add("has-stepper");

    const box = document.createElement("div");
    box.className = "num-steps";

    const mk = (dir, label, delta) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "num-step " + dir;
      b.tabIndex = -1;
      b.setAttribute("aria-label", label);
      // 按下时不抢输入框焦点：用户点两下箭头还能继续打字
      b.addEventListener("pointerdown", (ev) => ev.preventDefault());
      b.addEventListener("click", () => {
        const next = clamp(input, valueOf(input) + delta * stepOf(input));
        input.value = String(next);
        input.dispatchEvent(new Event("input", { bubbles: true }));
        if (typeof onChange === "function") onChange(next);
        else input.dispatchEvent(new Event("change", { bubbles: true }));
        syncButtons();
      });
      return b;
    };

    const up = mk("up", t("common.increase"), 1);
    const down = mk("down", t("common.decrease"), -1);
    box.append(up, down);
    host.appendChild(box);

    /// 到边界时禁用对应方向，避免「点了没反应」
    function syncButtons() {
      const v = valueOf(input);
      const min = bound(input, "min", -Infinity);
      const max = bound(input, "max", Infinity);
      up.disabled = v >= max;
      down.disabled = v <= min;
    }

    input.addEventListener("input", syncButtons);
    input.addEventListener("change", syncButtons);
    syncButtons();

    return input;
  }

  /// 批量：只处理页面上还没接过的 type=number。
  function scan(root) {
    for (const i of (root || document).querySelectorAll('input[type="number"]')) {
      enhance(i);
    }
  }

  return { enhance, scan };
})();

window.NumField = NumField;
export { NumField };
