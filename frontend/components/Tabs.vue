<script setup>
defineProps({
  modelValue: { type: [String, Number], default: "" },
  items: { type: Array, default: () => [] },
});

const emit = defineEmits(["update:modelValue"]);
</script>

<template>
  <div class="vx-tabs" role="tablist">
    <button
      v-for="item in items"
      :key="item.id"
      type="button"
      role="tab"
      :aria-selected="modelValue === item.id"
      :class="{ on: modelValue === item.id }"
      @click="emit('update:modelValue', item.id)"
    >
      {{ item.label }}
    </button>
  </div>
</template>

<style scoped>
.vx-tabs {
  display: flex;
  gap: var(--sp-4);
  border-bottom: 1px solid var(--border-subtle);
}
.vx-tabs button {
  padding: var(--sp-2) 0;
  border: 0;
  border-bottom: 2px solid transparent;
  background: none;
  color: var(--text-secondary);
  font-size: var(--fs-md);
  letter-spacing: var(--ls-normal);
  cursor: pointer;
}
.vx-tabs button:hover {
  color: var(--text-primary);
}
.vx-tabs button.on {
  color: var(--text-primary);
  border-bottom-color: var(--accent);
  font-weight: 600;
}
.vx-tabs button:focus-visible {
  box-shadow: var(--ring);
  outline: none;
}
</style>
