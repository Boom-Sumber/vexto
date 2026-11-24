import "../shared/contextmenu.js";
import { createVextoApp } from "../app.js";
import SettingsApp from "../settings/SettingsApp.vue";
import { useTheme } from "../composables/useTheme.js";
import { initI18n } from "../i18n/index.js";

await initI18n({ titleKey: "settings.title" });

const theme = useTheme();
await theme.init();

createVextoApp(SettingsApp).mount("#app");
