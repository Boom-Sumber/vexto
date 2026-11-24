import "../shared/contextmenu.js";
import "../shared/theme.js";
import "../shared/icons.js";
import "../shared/annot_editor.js";
import { createVextoApp } from "../app.js";
import PreviewApp from "../preview/App.vue";

createVextoApp(PreviewApp).mount("#app");
