<script setup>
import Icon from "../components/Icon.vue";
</script>

<template>
  <div id="screenshotView" class="main">
    <header class="head">
      <h1>{{ $t("main.navScreenshot") }}</h1>
    </header>

    <div class="page-body">
      <div id="shGallery" class="empty">
        <span class="empty-ico"><Icon name="camera" :size="30" /></span>
        <span>{{ $t("main.shEmptyGallery") }}</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.page-body { justify-content: center; }
.empty { flex: 1 1 auto; }
</style>
