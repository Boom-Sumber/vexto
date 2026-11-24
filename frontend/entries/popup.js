import "../shared/contextmenu.js";
import { createVextoApp } from "../app.js";
import App from "../popup/App.vue";
import { initI18n } from "../i18n/index.js";

await initI18n();

createVextoApp(App).mount("#app");
