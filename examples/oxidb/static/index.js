/* Vendored from zap (https://github.com/barisakin/zap), MIT, unmodified.
   The catalogue serves it, so the example stands on its own. */
// zap — a tiny, fast, fine-grained reactive UI library (no virtual DOM).
// Single entry point: re-exports the reactive core + the html template layer.
export {
  signal,
  store,
  computed,
  resource,
  effect,
  untrack,
  onCleanup,
  createRoot,
  h,
  setAttr,
  model,
  each,
  forEach,
  when,
  mergeProps,
  slot,
  createContext,
  provide,
  useContext,
  mount,
  hydrate,
} from "./zap.js";

export { html } from "./html.js";

export {
  Router,
  Link,
  navigate,
  useLocation,
  matchPath,
  configureRouter,
} from "./router.js";
