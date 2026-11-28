<script setup>
import { nextTick, ref, watch } from "vue";
import { useTabIndicator } from "../composables/useTabIndicator.js";

const props = defineProps({
  modelValue: { type: [String, Number], default: "" },
  /* item: { id, label, badge? } */
  items: { type: Array, default: () => [] },
});

const emit = defineEmits(["update:modelValue"]);

const wrap = ref(null);
const { style: indStyle, ready: indReady, update: updateInd } = useTabIndicator(wrap);

watch(
  () => [props.modelValue, props.items.map((i) => `${i.id}:${i.label}:${i.badge ?? ""}`).join("|")],
  () => nextTick(updateInd),
);
</script>

<template>
  <div ref="wrap" class="vx-tabs" role="tablist">
    <button
      v-for="item in items"
      :key="item.id"
      type="button"
      role="tab"
      :aria-selected="modelValue === item.id"
      :class="{ on: modelValue === item.id }"
      @click="emit('update:modelValue', item.id)"
    >
      <span class="vx-tabs-label">{{ item.label }}</span>
      <span v-if="item.badge !== undefined && item.badge !== null" class="vx-tabs-badge">{{ item.badge }}</span>
    </button>
    <span class="vx-tabs-ind" :class="{ animated: indReady }" :style="indStyle" aria-hidden="true"></span>
  </div>
</template>

<style scoped>
/* 与设置页 .tabs 同口径（styles/settings.css:255-278），这里是任务窗口的页内子导航。 */
.vx-tabs {
  position: relative;
  display: flex;
  gap: var(--sp-5);
  padding: 2px 0 0;
  box-shadow: inset 0 -1px 0 var(--border-default);
  overflow-x: auto;
  scrollbar-width: none;
}
.vx-tabs::-webkit-scrollbar { display: none; }

.vx-tabs button {
  flex: none;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-bottom: -1px;
  padding: 10px 2px;
  border: 0;
  background: none;
  color: var(--text-muted);
  font-family: inherit;
  font-size: var(--fs-md);
  letter-spacing: .02em;
  cursor: pointer;
  white-space: nowrap;
  transition: color var(--dur-fast) var(--ease-out);
}
.vx-tabs button:hover { color: var(--text-primary); }
.vx-tabs button.on {
  color: var(--text-primary);
  font-weight: 600;
}
.vx-tabs button:focus-visible {
  outline: none;
  box-shadow: var(--ring);
}

.vx-tabs-ind {
  position: absolute;
  left: 0;
  bottom: 0;
  height: 2px;
  border-radius: 1px;
  background: var(--accent);
  pointer-events: none;
  opacity: 0;
}
.vx-tabs-ind.animated {
  transition: transform 220ms var(--ease-out), width 220ms var(--ease-out);
}

.vx-tabs-badge {
  min-width: 18px;
  padding: 0 6px;
  border-radius: var(--r-pill);
  background: var(--bg-input);
  color: var(--text-dim);
  font-size: var(--fs-xs);
  font-weight: 500;
  text-align: center;
  font-variant-numeric: tabular-nums;
}
.vx-tabs button.on .vx-tabs-badge {
  background: var(--accent-soft-2);
  color: var(--accent);
}
</style>
