<script setup>
const props = defineProps({
  modelValue: Boolean,
  label: { type: String, default: "" },
  size: { type: String, default: "md" },
  disabled: Boolean,
});

const emit = defineEmits(["update:modelValue"]);
</script>

<template>
  <label class="vx-switch-row">
    <span v-if="label" class="vx-switch-label">{{ label }}</span>
    <button
      type="button"
      role="switch"
      :aria-checked="modelValue"
      :class="['vx-switch', size]"
      :disabled="disabled"
      @click="emit('update:modelValue', !modelValue)"
    >
      <span class="vx-switch-thumb" />
    </button>
  </label>
</template>

<style scoped>
.vx-switch-row {
  display: flex;
  align-items: center;
  gap: var(--sp-3);
}
.vx-switch-label {
  color: var(--text-secondary);
  font-size: var(--fs-md);
}
.vx-switch {
  position: relative;
  width: 44px;
  height: 24px;
  padding: 0;
  border: 1px solid var(--border-strong);
  border-radius: var(--r-pill);
  background: var(--bg-input);
  cursor: pointer;
  transition: background var(--dur-fast) var(--ease-out),
    border-color var(--dur-fast) var(--ease-out);
}
.vx-switch[aria-checked="true"] {
  background: var(--accent-soft);
  border-color: var(--accent);
}
.vx-switch:focus-visible {
  box-shadow: var(--ring);
  outline: none;
}
.vx-switch:disabled {
  opacity: 0.45;
  cursor: default;
}
.vx-switch-thumb {
  position: absolute;
  top: 3px;
  left: 3px;
  width: 16px;
  height: 16px;
  border-radius: 50%;
  background: var(--text-muted);
  transition: transform var(--dur-fast) var(--ease-out),
    background var(--dur-fast) var(--ease-out);
}
.vx-switch[aria-checked="true"] .vx-switch-thumb {
  transform: translateX(20px);
  background: var(--accent);
}
.vx-switch.sm {
  width: 32px;
  height: 18px;
}
.vx-switch.sm .vx-switch-thumb {
  top: 2px;
  left: 2px;
  width: 12px;
  height: 12px;
}
.vx-switch.sm[aria-checked="true"] .vx-switch-thumb {
  transform: translateX(14px);
}
</style>
