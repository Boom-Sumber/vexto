<script setup>
import Icon from "./Icon.vue";
import { useToast } from "../composables/useToast.js";
import "../styles/toast.css";

const { toasts, close, pause, resume } = useToast();

function iconName(kind) {
  return kind === "error" ? "alert" : kind === "success" ? "check" : "info";
}
</script>

<template>
  <div class="vx-toast-host" role="status" aria-live="polite">
    <div
      v-for="t in toasts"
      :key="t.id"
      class="vx-toast in"
      :class="t.kind"
      :role="t.kind === 'error' ? 'alert' : 'status'"
      @mouseenter="pause(t.id)"
      @mouseleave="resume(t.id)"
      @click="close(t.id)"
    >
      <span v-if="t.kind === 'loading'" class="vx-spinner" aria-hidden="true" />
      <span v-else class="vx-toast-ico">
        <Icon :name="iconName(t.kind)" :size="16" />
      </span>
      <span>{{ t.msg }}</span>
    </div>
  </div>
</template>
