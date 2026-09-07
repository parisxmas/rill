/* Vendored from zap (https://github.com/barisakin/zap), MIT, unmodified.
   The catalogue serves it, so the example stands on its own. */
// <Switch> — a themed on/off toggle bound to a boolean signal.
//   const on = signal(false);
//   Switch({ value: on, onChange: (v) => ... })
import { html } from "../index.js";

export function Switch(props) {
  const v = props.value; // Signal<boolean>
  const toggle = () => { v.set(!v()); props.onChange && props.onChange(v()); };
  return html`<button type="button" role="switch" aria-checked=${() => String(!!v())} class=${() => "zap-switch" + (v() ? " is-on" : "")} onclick=${toggle}><span class="zap-switch__dot"></span></button>`;
}
