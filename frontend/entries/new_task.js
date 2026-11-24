import "../shared/contextmenu.js";
import { createVextoApp } from "../app.js";
import App from "../new_task/App.vue";
import { useTheme } from "../composables/useTheme.js";
import { initI18n } from "../i18n/index.js";

await initI18n({ titleKey: "newTask.title" });

const theme = useTheme();
await theme.init();

createVextoApp(App).mount("#app");
