<script setup>
import { computed, reactive } from "vue";
import Icon from "../components/Icon.vue";
import { Icons } from "../shared/icons.js";

const props = defineProps({
  settings: { type: Object, required: true },
});

const busy = reactive(new Set());

const items = computed(() =>
  (props.settings.tools.value || []).filter((tool) => tool.switchable !== false),
);

async function toggle(tool, event) {
  if (busy.has(tool.id)) return;
  const want = event.target.checked;
  busy.add(tool.id);
  try {
    const ok = await props.settings.setToolEnabled(tool.id, want);
    if (!ok) event.target.checked = !want;
  } finally {
    busy.delete(tool.id);
  }
}
</script>

<template>
  <div class="tool-list-field">
    <div v-for="tool in items" :key="tool.id" class="tool-row">
      <span class="tool-ico">
        <Icon v-if="tool.icon && Icons.has(tool.icon)" :name="tool.icon" :size="18" />
        <span v-else class="dot"></span>
      </span>
      <div class="tool-meta">
        <div class="tool-name">{{ tool.name }}</div>
        <div class="tool-desc">{{ tool.desc }}</div>
      </div>
      <label class="switch-pill sm" :title="tool.name">
        <input
          type="checkbox"
          :checked="tool.enabled !== false"
          :disabled="busy.has(tool.id)"
          @change="toggle(tool, $event)"
        />
        <span class="track"></span>
        <span class="thumb"></span>
      </label>
    </div>
  </div>
</template>
