<script setup>
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import { toItems } from "./choiceOptions.js";

/** 自绘下拉：触发器和输入框同款，弹出面板由应用绘制（主题色边框、投影、
 *  对勾），不再弹系统白框。4 个以上选项用；2–3 个用 SegmentedControl。 */

const props = defineProps({
  modelValue: { type: [String, Number], default: "" },
  options: { type: Array, default: () => [] },
  disabled: Boolean,
  placeholder: { type: String, default: "" },
});
const emit = defineEmits(["update:modelValue"]);

const uid = "vxds-" + Math.random().toString(36).slice(2, 8);
const menuId = uid + "-menu";

const items = computed(() => toItems(props.options));
const selIndex = computed(() => {
  const v = String(props.modelValue ?? "");
  return items.value.findIndex((o) => o.value === v);
});
const label = computed(() =>
  selIndex.value >= 0 ? items.value[selIndex.value].label : props.placeholder,
);

const open = ref(false);
const active = ref(-1);
const up = ref(false);
const maxH = ref(244);

const root = ref(null);
const trigger = ref(null);

/** 可用的垂直范围：最近的滚动容器（设置页是 .pane-body）或整个视口。
 *  只按 window 算的话，面板会以为底部还有被页脚占掉的空间。 */
function boxOf(el) {
  let node = el.parentElement;
  while (node) {
    const oy = getComputedStyle(node).overflowY;
    if (oy === "auto" || oy === "scroll") return node.getBoundingClientRect();
    node = node.parentElement;
  }
  return { top: 0, bottom: window.innerHeight };
}

function place() {
  const t = trigger.value;
  if (!t) return;
  const r = t.getBoundingClientRect();
  const box = boxOf(t);
  const below = box.bottom - r.bottom - 10;
  const above = r.top - box.top - 10;
  up.value = below < 200 && above > below;
  maxH.value = Math.max(120, Math.min(244, up.value ? above : below));
}

/** 只滚动面板自身，不让 scrollIntoView 把整个设置页滚走——
 *  页面一滚，面板顶部就会被滚动容器裁掉。 */
function scrollActive() {
  const menu = root.value?.querySelector(".menu");
  const opt = root.value?.querySelector(".opt[data-active='true']");
  if (!menu || !opt) return;
  const top = opt.offsetTop;
  const bottom = top + opt.offsetHeight;
  if (top < menu.scrollTop) menu.scrollTop = top - 4;
  else if (bottom > menu.scrollTop + menu.clientHeight) {
    menu.scrollTop = bottom - menu.clientHeight + 4;
  }
}

function show() {
  if (props.disabled || !items.value.length) return;
  active.value = selIndex.value >= 0 ? selIndex.value : 0;
  place();
  open.value = true;
  nextTick(() => {
    scrollActive();
    place();
    document.addEventListener("pointerdown", onPointerDown, true);
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
  });
}

function hide(focus) {
  if (!open.value) return;
  open.value = false;
  document.removeEventListener("pointerdown", onPointerDown, true);
  window.removeEventListener("resize", place);
  window.removeEventListener("scroll", place, true);
  if (focus) trigger.value?.focus();
}

function onPointerDown(e) {
  if (!root.value?.contains(e.target)) hide(false);
}

function pick(i) {
  const it = items.value[i];
  if (!it) return;
  // 选中项与当前值相同就不提交：设置页的 commit 会触发保存
  if (it.value !== String(props.modelValue ?? "")) emit("update:modelValue", it.value);
  hide(true);
}

function toggle() {
  open.value ? hide(false) : show();
}

function move(step) {
  const n = items.value.length;
  if (!n) return;
  const from = active.value < 0 ? 0 : active.value;
  active.value = (from + step + n) % n;
  nextTick(scrollActive);
}

function onKey(e) {
  if (!open.value) {
    if (["ArrowDown", "ArrowUp", "Enter", " ", "Home", "End"].includes(e.key)) {
      e.preventDefault();
      show();
      if (e.key === "ArrowUp" || e.key === "End") active.value = items.value.length - 1;
    }
    return;
  }
  switch (e.key) {
    case "ArrowDown": e.preventDefault(); move(1); break;
    case "ArrowUp": e.preventDefault(); move(-1); break;
    case "Home": e.preventDefault(); active.value = 0; break;
    case "End": e.preventDefault(); active.value = items.value.length - 1; break;
    case "Enter":
    case " ": e.preventDefault(); pick(active.value); break;
    case "Escape": e.preventDefault(); hide(true); break;
    case "Tab": hide(false); break;
  }
}

// 选项变了（列表增删 / 引擎增删）时把高亮夹回范围
watch(items, (list) => {
  if (active.value >= list.length) active.value = Math.max(0, list.length - 1);
  if (!list.length) hide(false);
});

onBeforeUnmount(() => hide(false));
</script>

<template>
  <div
    ref="root"
    class="select"
    :class="{ open, up, disabled }"
    @keydown="onKey"
  >
    <button
      ref="trigger"
      type="button"
      class="trigger"
      :disabled="disabled"
      aria-haspopup="listbox"
      :aria-expanded="open ? 'true' : 'false'"
      :aria-controls="menuId"
      :aria-activedescendant="open && active >= 0 ? `${uid}-opt-${active}` : undefined"
      @click="toggle"
    >
      <span class="label" :class="{ dim: selIndex < 0 }">{{ label }}</span>
      <svg class="chev" viewBox="0 0 12 12" fill="none" aria-hidden="true">
        <path
          d="M2.5 4.75 6 8.25l3.5-3.5"
          stroke="currentColor"
          stroke-width="1.5"
          stroke-linecap="round"
          stroke-linejoin="round"
        />
      </svg>
    </button>

    <Transition name="menu">
      <div
        v-show="open"
        :id="menuId"
        class="menu"
        role="listbox"
        :style="{ '--menu-max': maxH + 'px' }"
      >
        <button
          v-for="(it, i) in items"
          :id="`${uid}-opt-${i}`"
          :key="it.value"
          type="button"
          class="opt"
          role="option"
          :aria-selected="i === selIndex ? 'true' : 'false'"
          :data-active="i === active ? 'true' : null"
          @mouseenter="active = i"
          @click="pick(i)"
        >
          <span class="txt">{{ it.label }}</span>
          <svg class="tick" viewBox="0 0 14 14" fill="none" aria-hidden="true">
            <path
              d="m2.5 7.5 3 3 6-6.5"
              stroke="currentColor"
              stroke-width="1.8"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
          </svg>
        </button>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.select { position: relative; }

.trigger {
  position: relative;
  display: flex;
  align-items: center;
  width: 100%;
  height: var(--h-control);
  padding: 0 30px 0 var(--sp-3);
  background: var(--bg-input);
  border: 1px solid var(--border-default);
  border-radius: var(--r-md);
  color: var(--text-primary);
  font: inherit;
  font-size: var(--fs-md);
  text-align: left;
  cursor: pointer;
  transition: border-color var(--dur-fast) var(--ease-out),
              background-color var(--dur-fast) var(--ease-out),
              box-shadow var(--dur-fast) var(--ease-out);
}
.trigger:hover { border-color: var(--border-strong); }
.trigger:focus-visible {
  outline: none;
  border-color: var(--accent);
  box-shadow: var(--ring);
}
.select.open .trigger {
  background: var(--bg-panel);
  border-color: var(--accent);
  box-shadow: var(--ring);
}
.select.disabled .trigger { opacity: .5; cursor: default; }

.label {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.label.dim { color: var(--text-disabled); }

.chev {
  position: absolute;
  right: 9px;
  top: 50%;
  width: 12px;
  height: 12px;
  margin-top: -6px;
  color: var(--text-dim);
  pointer-events: none;
  transition: transform var(--dur-normal) var(--ease-out),
              color var(--dur-fast) var(--ease-out);
}
.select.open .chev { transform: rotate(180deg); color: var(--accent); }

/* 面板右对齐触发器（控件列在设置页右侧，向左展开不会越界） */
.menu {
  position: absolute;
  top: calc(100% + 6px);
  right: 0;
  z-index: var(--z-popup);
  min-width: 100%;
  width: max-content;
  max-width: 300px;
  max-height: var(--menu-max, 244px);
  overflow: auto;
  padding: var(--sp-1);
  background: var(--menu-bg);
  border-radius: var(--r-lg);
  box-shadow: var(--menu-shadow);
  transform-origin: top right;
}
.select.up .menu {
  top: auto;
  bottom: calc(100% + 6px);
  transform-origin: bottom right;
}

.opt {
  display: flex;
  align-items: center;
  gap: var(--sp-2);
  width: 100%;
  height: 30px;
  padding: 0 var(--sp-2);
  border: 0;
  border-radius: var(--r-sm);
  background: none;
  color: var(--text-secondary);
  font: inherit;
  font-size: var(--fs-md);
  text-align: left;
  white-space: nowrap;
  cursor: pointer;
  transition: background-color var(--dur-fast) var(--ease-out),
              color var(--dur-fast) var(--ease-out);
}
.txt {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}
.tick {
  flex: none;
  width: 14px;
  height: 14px;
  color: var(--menu-tick);
  opacity: 0;
}
.opt[data-active="true"] { background: var(--menu-row-hover); }
.opt[aria-selected="true"] {
  background: var(--bg-selected);
  color: var(--text-primary);
  font-weight: 600;
}
.opt[aria-selected="true"] .tick { opacity: 1; }
.opt[aria-selected="true"][data-active="true"] { background: var(--accent-soft); }

.menu-enter-active,
.menu-leave-active {
  transition: opacity var(--dur-fast) var(--ease-out),
              transform var(--dur-normal) var(--ease-out);
}
.menu-enter-from,
.menu-leave-to {
  opacity: 0;
  transform: translateY(-4px) scale(.985);
}
</style>
