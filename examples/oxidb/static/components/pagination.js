/* Vendored from zap (https://github.com/barisakin/zap), MIT, unmodified.
   The catalogue serves it, so the example stands on its own. */
// <Pagination> — standalone pager with first/last + windowed numbers + ellipsis.
//   const page = signal(1);
//   Pagination({ page, totalPages: 12, onChange: (p) => ... })   // totalPages: number | getter
import { computed, html, forEach } from "../index.js";

export function Pagination(props) {
  const page = props.page;
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
      if (p === 1 || p === t || Math.abs(p - c) <= 1) out.push(p);
      else if (out[out.length - 1] !== "…") out.push("…");
    }
    return out.map((p, i) => ({ id: p === "…" ? "gap" + i : p, p }));
  });

  const numBtn = ({ p }) =>
    p === "…"
      ? html`<span class="zap-pg__gap">…</span>`
      : html`<button type="button" class=${() => "zap-pg__num" + (page() === p ? " is-active" : "")} onclick=${() => go(p)}>${p}</button>`;

  return html`<div class="zap-pg"><button type="button" class="zap-pg__num" disabled=${() => page() <= 1} onclick=${() => go(page() - 1)}>‹</button>${forEach(cells, numBtn)}<button type="button" class="zap-pg__num" disabled=${() => page() >= getTotal()} onclick=${() => go(page() + 1)}>›</button></div>`;
}
