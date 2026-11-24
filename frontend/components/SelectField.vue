<script setup>
import { computed } from "vue";
import DropdownSelect from "./DropdownSelect.vue";
import SegmentedControl from "./SegmentedControl.vue";
import { preferSegmented, toItems } from "./choiceOptions.js";

const props = defineProps({
  modelValue: { type: [String, Number], default: "" },
  label: { type: String, default: "" },
  options: { type: Array, default: () => [] },
  disabled: Boolean,
});

const emit = defineEmits(["update:modelValue"]);

const items = computed(() => toItems(props.options));
const segmented = computed(() => preferSegmented(items.value));
</script>

<template>
  <label class="vx-field" :class="{ 'no-label': !label }">
    <span v-if="label" class="vx-field-label">{{ label }}</span>
    <span class="vx-field-body">
      <SegmentedControl
        v-if="segmented"
        :model-value="modelValue"
        :options="options"
        :disabled="disabled"
        @update:model-value="emit('update:modelValue', $event)"
      />
      <DropdownSelect
        v-else
        :model-value="modelValue"
        :options="options"
        :disabled="disabled"
        @update:model-value="emit('update:modelValue', $event)"
      />
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
