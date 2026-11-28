import { nextTick, onBeforeUnmount, onMounted, ref } from "vue";

/**
 * 标签栏选中态横线的滑动位置。
 *
 * 用法：容器 `position: relative`，把横线作为容器的绝对定位子元素，
 * 标签按钮带 `aria-selected`。返回的 `style` 绑到横线上；`ready` 为 false
 * 时样式应禁用过渡，避免首次挂载时横线从 0 宽滑入。
 */
export function useTabIndicator(wrapRef) {
  const style = ref({ width: "0px", transform: "translateX(0px)", opacity: "0" });
  const ready = ref(false);
  let ro = null;

  function update() {
    const wrap = wrapRef.value;
    if (!wrap) return;
    const btn = wrap.querySelector('[aria-selected="true"]');
    if (!btn) {
      style.value = { width: "0px", transform: "translateX(0px)", opacity: "0" };
      return;
    }
    style.value = {
      width: btn.offsetWidth + "px",
      transform: `translateX(${btn.offsetLeft}px)`,
      opacity: "1",
    };
    if (!ready.value) requestAnimationFrame(() => (ready.value = true));
  }

  onMounted(() => {
    nextTick(update);
    if (typeof ResizeObserver !== "undefined") {
      ro = new ResizeObserver(update);
      ro.observe(wrapRef.value);
    }
    window.addEventListener("resize", update);
  });

  onBeforeUnmount(() => {
    ro?.disconnect();
    window.removeEventListener("resize", update);
  });

  return { style, ready, update };
}
