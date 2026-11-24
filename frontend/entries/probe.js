import "../probe/mock.js";
import "../shared/contextmenu.js";
import { createVextoApp } from "../app.js";
import App from "../probe/App.vue";
import { useTheme } from "../composables/useTheme.js";
import { initI18n } from "../i18n/index.js";

await initI18n();

const theme = useTheme();
await theme.init();

createVextoApp(App).mount("#app");
