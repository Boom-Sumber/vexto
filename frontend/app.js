import { createApp } from "vue";
import { installI18n } from "./i18n/index.js";

/** 所有 Vue 入口统一的 createApp：安装 $t，未来其它全局能力也走这里。 */
export function createVextoApp(rootComponent) {
  const app = createApp(rootComponent);
  installI18n(app);
  return app;
}
