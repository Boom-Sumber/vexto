<script setup>
import { vNumField } from "../directives/numField.js";

defineOptions({ inheritAttrs: false });

const props = defineProps({
  modelValue: { type: [String, Number], default: "" },
  label: { type: String, default: "" },
  type: { type: String, default: "text" },
  placeholder: { type: String, default: "" },
  disabled: Boolean,
  min: { type: [String, Number], default: undefined },
  max: { type: [String, Number], default: undefined },
  step: { type: [String, Number], default: undefined },
  help: { type: String, default: "" },
  stepper: Boolean,
});

const emit = defineEmits(["update:modelValue"]);

function onInput(ev) {
  const raw = ev.target.value;
  emit("update:modelValue", props.type === "number" && raw !== "" ? Number(raw) : raw);
}
</script>

<template>
  <label class="vx-field" :class="{ 'no-label': !label }">
    <span v-if="label" class="vx-field-label">{{ label }}</span>
    <span class="vx-field-body">
      <input
        v-if="stepper"
        v-num-field
        :type="type"
        :value="modelValue"
        :placeholder="placeholder"
        :disabled="disabled"
        :min="min"
        :max="max"
        :step="step"
        v-bind="$attrs"
        @input="onInput"
      />
      <input
        v-else
        :type="type"
        :value="modelValue"
        :placeholder="placeholder"
        :disabled="disabled"
        :min="min"
        :max="max"
        :step="step"
        v-bind="$attrs"
        @input="onInput"
      />
      <span v-if="help" class="vx-help">{{ help }}</span>
    </span>
  </label>
</template>

<style scoped>
.vx-field {
  display: grid;
  grid-template-columns: var(--w-field-label) minmax(0, 1fr);
  column-gap: var(--sp-3);
  row-gap: var(--sp-1);
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
  display: flex;
  flex-direction: column;
  gap: var(--sp-1);
  min-width: 0;
}
.vx-help {
  color: var(--text-muted);
  font-size: var(--fs-xs);
  line-height: var(--lh-normal);
}
</style>
