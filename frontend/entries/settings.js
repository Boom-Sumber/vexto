import "../shared/contextmenu.js";
import { createVextoApp } from "../app.js";
import SettingsApp from "../settings/SettingsApp.vue";
import { useTheme } from "../composables/useTheme.js";
import { t } from "../i18n/index.js";

const theme = useTheme();
await theme.init();

document.title = t("settings.title");
createVextoApp(SettingsApp).mount("#app");
