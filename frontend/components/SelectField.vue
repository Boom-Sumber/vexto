<script setup>
const props = defineProps({
  modelValue: { type: [String, Number], default: "" },
  label: { type: String, default: "" },
  options: { type: Array, default: () => [] },
  disabled: Boolean,
});

const emit = defineEmits(["update:modelValue"]);
</script>

<template>
  <label class="vx-field" :class="{ 'no-label': !label }">
    <span v-if="label" class="vx-field-label">{{ label }}</span>
    <span class="vx-field-body">
      <select
        :value="modelValue"
        :disabled="disabled"
        @change="emit('update:modelValue', $event.target.value)"
      >
        <option v-for="opt in options" :key="opt.value" :value="opt.value">
          {{ opt.label }}
        </option>
      </select>
    </span>
  </label>
</template>

<style scoped>
.vx-field {
  display: grid;
  grid-template-columns: var(--w-field-label) minmax(0, 1fr);
  column-gap: var(--sp-3);
  align-items: center;
}
.vx-field.no-label {
  grid-template-columns: minmax(0, 1fr);
}
.vx-field-label {
  color: var(--text-secondary);
  font-size: var(--fs-md);
}
.vx-field-body {
  min-width: 0;
}
</style>
