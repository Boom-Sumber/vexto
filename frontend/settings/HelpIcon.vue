<script setup>
import Icon from "../components/Icon.vue";
import { hideHelp, showHelp } from "./helpTip.js";

defineProps({ text: { type: String, default: "" } });
</script>

<template>
  <span
    class="field-help"
    tabindex="0"
    role="note"
    :aria-label="$t('settings.help')"
    @mouseenter="text && showHelp($event.currentTarget, text)"
    @mouseleave="hideHelp"
    @focus="text && showHelp($event.currentTarget, text)"
    @blur="hideHelp"
    @click.stop
  >
    <Icon name="help" :size="14" />
  </span>
</template>
