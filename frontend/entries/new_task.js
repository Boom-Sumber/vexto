import "../shared/contextmenu.js";
import { createApp } from "vue";
import App from "../new_task/App.vue";
import { useTheme } from "../composables/useTheme.js";

const theme = useTheme();
await theme.init();

createApp(App).mount("#app");
