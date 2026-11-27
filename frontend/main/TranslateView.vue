<script setup>
import { computed, ref } from "vue";
import Icon from "../components/Icon.vue";
import Tabs from "../components/Tabs.vue";
import { t } from "../i18n/index.js";

const sub = ref("translate");

const items = computed(() => [
  { id: "translate", label: t("main.trTabTranslate") },
  { id: "history", label: t("main.trTabHistory") },
]);
</script>

<template>
  <div id="translateView" class="main">
    <div class="view-tabs">
      <Tabs v-model="sub" :items="items" />
    </div>

    <header class="head">
      <h1>{{ $t("main.navTranslate") }}</h1>
    </header>

    <div class="page-body">
      <div v-show="sub === 'translate'" id="trPaneTranslate" class="empty">
        <span class="empty-ico"><Icon name="translate" :size="30" /></span>
        <span>{{ $t("main.trEmptyTranslate") }}</span>
      </div>
      <div v-show="sub === 'history'" id="trPaneHistory" class="empty">
        <span class="empty-ico"><Icon name="history" :size="30" /></span>
        <span>{{ $t("main.trEmptyHistory") }}</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.page-body { justify-content: center; }
.empty { flex: 1 1 auto; }
</style>
