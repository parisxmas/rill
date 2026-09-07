/* Vendored from zap (https://github.com/barisakin/zap), MIT, unmodified.
   The catalogue serves it, so the example stands on its own. */
// <Button> — a themed zap component. Pure styling via the theme tokens
// (.zap-btn / variants in theme.css); zero hard-coded colours.
//
//   import { Button } from "./components/button.js";
//   Button({ label: "Save", onClick: save })
//   Button({ variant: "ghost", children: html`<b>Cancel</b>`, onClick: close })

import { html, mergeProps } from "../index.js";

export function Button(props) {
  props = mergeProps({ variant: "primary", type: "button" }, props);
  return html`
    <button type=${props.type}
            class=${() => "zap-btn zap-btn--" + props.variant}
            disabled=${props.disabled ?? false}
            onclick=${props.onClick}>${props.children ?? props.label}</button>`;
}
