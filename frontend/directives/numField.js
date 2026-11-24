import { NumField } from "../shared/controls.js";

export const vNumField = {
  mounted(el) {
    NumField.enhance(el);
  },
};
