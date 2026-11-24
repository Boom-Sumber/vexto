import { readonly, ref } from "vue";

const toasts = ref([]);
const timers = new Map();
let seq = 0;

function defaultDuration(kind) {
  return kind === "error" ? 6000 : kind === "success" ? 2000 : kind === "loading" ? 0 : 3000;
}

function remove(id) {
  const timer = timers.get(id);
  if (timer) clearTimeout(timer);
  timers.delete(id);
  toasts.value = toasts.value.filter((t) => t.id !== id);
}

function arm(id) {
  const item = toasts.value.find((t) => t.id === id);
  if (!item) return;
  const old = timers.get(id);
  if (old) clearTimeout(old);
  timers.delete(id);
  if (item.duration > 0) {
    timers.set(id, setTimeout(() => remove(id), item.duration));
  }
}

function push(msg, opts = {}) {
  if (!msg) return null;
  const kind = opts.kind || "info";
  const item = {
    id: ++seq,
    msg,
    kind,
    duration: typeof opts.duration === "number" ? opts.duration : defaultDuration(kind),
  };
  toasts.value.push(item);
  while (toasts.value.length > 3) remove(toasts.value[0].id);
  arm(item.id);
  return {
    id: item.id,
    update(nextMsg, nextOpts = {}) {
      const current = toasts.value.find((t) => t.id === item.id);
      if (!current) return this;
      if (nextMsg) current.msg = nextMsg;
      if (nextOpts.kind) current.kind = nextOpts.kind;
      current.duration =
        typeof nextOpts.duration === "number"
          ? nextOpts.duration
          : nextOpts.kind
            ? defaultDuration(nextOpts.kind)
            : current.duration;
      arm(item.id);
      return this;
    },
    close() {
      remove(item.id);
    },
  };
}

function pause(id) {
  const timer = timers.get(id);
  if (timer) clearTimeout(timer);
  timers.delete(id);
}

function resume(id) {
  arm(id);
}

export function useToast() {
  return {
    toasts: readonly(toasts),
    show: push,
    loading: (msg) => push(msg, { kind: "loading", duration: 0 }),
    close: remove,
    pause,
    resume,
  };
}
