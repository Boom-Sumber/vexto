import "../shared/contextmenu.js";
import { createVextoApp } from "../app.js";
import App from "../feedback/App.vue";
import { useTheme } from "../composables/useTheme.js";
import { initI18n } from "../i18n/index.js";

await initI18n({ titleKey: "feedback.windowTitle" });

const theme = useTheme();
await theme.init();

createVextoApp(App).mount("#app");
