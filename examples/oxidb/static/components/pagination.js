/* Vendored from zap (https://github.com/barisakin/zap), MIT, unmodified.
   The catalogue serves it, so the example stands on its own. */
// <Pagination> — standalone pager: first and last, a step either way, and a
// window of numbers around the one you are on with ellipses for the rest.
//   const page = signal(1);
//   Pagination({ page, totalPages: 12, onChange: (p) => ... })   // totalPages: number | getter
//   Pagination({ page, totalPages, span: 3 })   // how many numbers either side (default 2)
import { computed, html, forEach } from "../index.js";

export function Pagination(props) {
  const page = props.page;
  const span = props.span ?? 2;
  const getTotal = typeof props.totalPages === "function" ? props.totalPages : () => props.totalPages || 1;

  const go = (p) => {
    const t = getTotal();
    const n = Math.max(1, Math.min(t, p));
    if (n !== page()) { page.set(n); props.onChange && props.onChange(n); }
  };

  // 1 … (c-1) c (c+1) … last
  const cells = computed(() => {
    const t = getTotal(), c = page(), out = [];
    for (let p = 1; p <= t; p++) {
      if (p === 1 || p === t || Math.abs(p - c) <= span) out.push(p);
      else if (out[out.length - 1] !== "…") out.push("…");
    }
    return out.map((p, i) => ({ id: p === "…" ? "gap" + i : p, p }));
  });

  const numBtn = ({ p }) =>
    p === "…"
      ? html`<span class="zap-pg__gap">…</span>`
      : html`<button type="button" class=${() => "zap-pg__num" + (page() === p ? " is-active" : "")} onclick=${() => go(p)}>${p}</button>`;

  const step = (label, to, spent) =>
    html`<button type="button" class="zap-pg__num zap-pg__step" title=${label.title} disabled=${spent} onclick=${() => go(to())}>${label.sign}</button>`;

  return html`<div class="zap-pg">${step({ sign: "«", title: "First" }, () => 1, () => page() <= 1)}${step(
    { sign: "‹", title: "Previous" },
    () => page() - 1,
    () => page() <= 1,
  )}${forEach(cells, numBtn)}${step({ sign: "›", title: "Next" }, () => page() + 1, () => page() >= getTotal())}${step(
    { sign: "»", title: "Last" },
    () => getTotal(),
    () => page() >= getTotal(),
  )}</div>`;
}
