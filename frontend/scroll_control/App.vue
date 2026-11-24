<script setup>
import { onMounted, ref } from "vue";
import { startScrollControl } from "./controller.js";

const root = ref(null);
onMounted(() => startScrollControl(root.value));
</script>

<template>
  <div ref="root" class="scr">
    <div class="scr-head">
      <span class="scr-title">{{ $t("scroll.title") }}</span>
      <span id="scr-badge" class="scr-badge">{{ $t("scroll.running") }}</span>
    </div>
    <div id="scr-line" class="scr-line">{{ $t("scroll.capturing") }}</div>
    <div class="scr-bar"><div id="scr-fill" class="scr-fill"></div></div>
    <div class="scr-acts">
      <button id="scr-preview" class="btn" type="button" hidden>{{ $t("region.preview") }}</button>
      <button id="scr-pin" class="btn" type="button" hidden>{{ $t("region.pin") }}</button>
      <button id="scr-stop" class="btn" type="button">{{ $t("scroll.stop") }}</button>
      <button id="scr-close" class="btn btn-primary" type="button" hidden>{{ $t("common.close") }}</button>
    </div>
  </div>
</template>
