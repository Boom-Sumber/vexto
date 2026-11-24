<script setup>
import { computed, nextTick, ref } from "vue";
import { toItems } from "./choiceOptions.js";

/** 2–3 个选项的选择控件：矩形轨道里排选项，滑块平移到选中项。
 *  视觉与输入框同高（32px），凹槽用 --seg-track，滑块 --seg-thumb。 */

const props = defineProps({
  modelValue: { type: [String, Number], default: "" },
  options: { type: Array, default: () => [] },
  disabled: Boolean,
});
const emit = defineEmits(["update:modelValue"]);

const items = computed(() => toItems(props.options));
const cur = computed(() => {
  const v = String(props.modelValue ?? "");
  const i = items.value.findIndex((o) => o.value === v);
  return i < 0 ? 0 : i;
});
const vars = computed(() => ({ "--n": items.value.length || 1, "--i": cur.value }));

const root = ref(null);

function pick(i) {
  const it = items.value[i];
  if (!it || props.disabled || it.value === String(props.modelValue ?? "")) return;
  emit("update:modelValue", it.value);
}

/** 单选组的键盘约定：方向键移动并立即选中，Home / End 跳两端。 */
function onKey(e) {
  const n = items.value.length;
  if (!n) return;
  let next = null;
  if (e.key === "ArrowRight" || e.key === "ArrowDown") next = (cur.value + 1) % n;
  else if (e.key === "ArrowLeft" || e.key === "ArrowUp") next = (cur.value - 1 + n) % n;
  else if (e.key === "Home") next = 0;
  else if (e.key === "End") next = n - 1;
  else if (e.key === " " || e.key === "Enter") {
    e.preventDefault();
    pick(cur.value);
    return;
  }
  if (next == null) return;
  e.preventDefault();
  pick(next);
  nextTick(() => root.value?.querySelectorAll("button")[next]?.focus());
}
</script>

<template>
  <div
    ref="root"
    class="seg"
    :class="{ disabled }"
    role="radiogroup"
    :style="vars"
    @keydown="onKey"
  >
    <span class="thumb" aria-hidden="true"></span>
    <button
      v-for="(it, i) in items"
      :key="it.value"
      type="button"
      role="radio"
      :aria-checked="i === cur ? 'true' : 'false'"
      :tabindex="i === cur ? 0 : -1"
      :title="it.title || it.label"
      :disabled="disabled"
      @click="pick(i)"
    >
      {{ it.label }}
    </button>
  </div>
</template>

<style scoped>
/* 1px 边框 + 3px 内缩 + 24px 按钮 = 32px，与 .input / 下拉触发器同高 */
.seg {
  position: relative;
  display: flex;
  padding: 3px;
  background: var(--seg-track);
  border: 1px solid var(--border-default);
  border-radius: var(--r-md);
  transition: border-color var(--dur-fast) var(--ease-out);
}
.seg:hover { border-color: var(--border-strong); }
.seg:focus-within { border-color: var(--accent); box-shadow: var(--ring); }
.seg.disabled { opacity: .5; pointer-events: none; }

.thumb {
  position: absolute;
  top: 3px;
  left: 3px;
  bottom: 3px;
  width: calc((100% - 6px) / var(--n));
  background: var(--seg-thumb);
  border-radius: calc(var(--r-md) - 2px);
  box-shadow: var(--seg-thumb-shadow);
  transform: translateX(calc(var(--i) * 100%));
  transition: transform var(--dur-normal) var(--ease-out);
  pointer-events: none;
}

.seg button {
  position: relative;
  z-index: 1;
  flex: 1 1 0;
  min-width: 0;
  height: 24px;
  padding: 0 6px;
  border: 0;
  border-radius: calc(var(--r-md) - 2px);
  background: none;
  font: inherit;
  font-size: var(--fs-sm);
  font-weight: 500;
  color: var(--text-muted);
  letter-spacing: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  cursor: pointer;
  transition: color var(--dur-fast) var(--ease-out);
}
.seg button:hover { color: var(--text-secondary); }
.seg button[aria-checked="true"] { color: var(--text-primary); font-weight: 600; }
.seg button:focus-visible { outline: none; }
</style>
