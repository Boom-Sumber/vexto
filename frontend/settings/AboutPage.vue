<script setup>
import { nextTick, onMounted, ref, watch } from "vue";
import Icon from "../components/Icon.vue";
import UpdatePanel from "./UpdatePanel.vue";
import { Icons } from "../shared/icons.js";
import { errText } from "./utils.js";

const props = defineProps({
  field: { type: Object, required: true },
  settings: { type: Object, required: true },
});

const update = props.settings.update;
const info = ref(update.info.value);
const error = ref("");
const updateBlock = ref(null);

function scrollToUpdate() {
  if (!update.focusToken.value) return;
  nextTick(() => {
    updateBlock.value?.scrollIntoView({ block: "center", behavior: "smooth" });
  });
}

onMounted(async () => {
  try {
    info.value = await update.loadInfo();
  } catch (e) {
    error.value = errText(e);
  }
  scrollToUpdate();
});

watch(() => update.focusToken.value, scrollToUpdate);
</script>

<template>
  <div class="about-page">
    <p v-if="error" class="about-text">{{ $t("settings.aboutReadFailed", { message: error }) }}</p>

    <template v-else-if="info">
      <template v-if="field.part !== 'legal'">
        <div class="about-hero">
          <span class="about-logo logo-badge"></span>
          <div>
            <div class="about-name-row">
              <h1 class="about-name">{{ info.name }}</h1>
              <span class="about-version">v{{ info.version }}</span>
            </div>
            <div class="about-subtitle">{{ info.subtitle }}</div>
          </div>
        </div>

        <p class="about-intro">{{ info.intro }}</p>

        <div class="about-features">
          <div v-for="feat in info.features || []" :key="feat.title" class="about-feature">
            <span class="about-feature-icon">
              <Icon v-if="Icons.has(feat.icon)" :name="feat.icon" :size="20" />
              <template v-else>·</template>
            </span>
            <div>
              <div class="about-feature-title">{{ feat.title }}</div>
              <div class="about-feature-desc">{{ feat.desc }}</div>
            </div>
          </div>
        </div>

        <div class="about-links">
          <button
            v-for="link in info.links || []"
            :key="link.id"
            class="about-link"
            type="button"
            @click="update.openLink(link.id)"
          >
            {{ link.label }}
          </button>
        </div>

        <div ref="updateBlock">
          <UpdatePanel :update="update" />
        </div>
      </template>

      <template v-else>
        <hr class="about-divider" />
        <h2 class="about-section-title">{{ $t("settings.aboutLicenseTitle") }}</h2>
        <p class="about-text">{{ info.license }}</p>
        <p class="about-text about-attribution">{{ info.attribution }}</p>

        <div class="about-actions">
          <button class="btn" type="button" @click="update.openLink('license')">
            {{ $t("settings.aboutViewLicense") }}
          </button>
        </div>

        <details class="about-components" open>
          <summary>
            <span>{{ $t("settings.aboutComponents") }}</span>
            <span class="about-chevron"></span>
          </summary>
          <table>
            <thead>
              <tr>
                <th>{{ $t("settings.aboutColComponent") }}</th>
                <th>{{ $t("settings.aboutColVersion") }}</th>
                <th>{{ $t("settings.aboutColLicense") }}</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="c in info.components || []" :key="c.name">
                <td>{{ c.name }}</td>
                <td>{{ c.version }}</td>
                <td>{{ c.license }}</td>
              </tr>
            </tbody>
          </table>
        </details>

        <footer class="about-footer">{{ info.footer }}</footer>
      </template>
    </template>
  </div>
</template>
