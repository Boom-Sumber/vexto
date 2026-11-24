<script setup>
import { computed } from "vue";

const props = defineProps({
  variant: { type: String, default: "secondary" },
  type: { type: String, default: "button" },
  disabled: Boolean,
});

const cls = computed(() => [
  "btn",
  props.variant === "primary" ? "btn-primary" : "",
  props.variant === "ghost" ? "btn-ghost" : "",
]);
</script>

<template>
  <button :type="type" :class="cls" :disabled="disabled">
    <slot />
  </button>
</template>
