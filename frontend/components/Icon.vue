<script setup>
import { computed } from "vue";
import { Icons } from "../shared/icons.js";

const props = defineProps({
  name: { type: String, required: true },
  size: { type: Number, default: 16 },
});

const html = computed(() => (Icons.has(props.name) ? Icons.svg(props.name, props.size) : ""));
</script>

<template>
  <span class="vx-icon" aria-hidden="true" v-html="html" />
</template>

<style scoped>
.vx-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  line-height: 0;
  flex: none;
}
</style>
