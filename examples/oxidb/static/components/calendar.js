/* Vendored from zap (https://github.com/barisakin/zap), MIT, unmodified.
   The catalogue serves it, so the example stands on its own. */
// <Calendar> — a themed, fine-grained date picker built on the zap engine.
// Styling is 100% theme tokens (.zap-cal* in theme.css), so it follows whatever
// theme is active with zero JS.
//
// Props:
//   value?:    Signal<Date|null>   selected date (read + written). Omit for internal state.
//   onSelect?: (date: Date) => void
//   weekStart?: 0 (Sun) | 1 (Mon)  default 1 (Mon)
//   locale?:   "en" | "tr"          month/weekday names (default "en"). Or pass
//              custom `months` (12) / `weekdays` (7, Sunday-first) arrays to override.
//   disabled?: (date: Date) => boolean   days for which this returns true can't be
//              selected (e.g. booked or past dates). They get the `is-disabled`
//              class and ignore clicks.
//   range?:    boolean                   range mode: first click sets the start, the
//              next sets the end. A disabled (booked) day between them clamps the end,
//              so a range can never span a booked night.
//   start?, end?: Signal<Date|null>      range endpoints (read + written). Omit for internal.
//   onRange?:  (start: Date, end: Date) => void
//
//   import { Calendar } from "./components/calendar.js";
//   const picked = signal(null);
//   const booked = new Set(["2026-06-15", "2026-06-16"]);
//   const iso = (d) => d.toISOString().slice(0, 10);
//   Calendar({ value: picked, disabled: (d) => booked.has(iso(d)), onSelect: d => console.log(d) })
//
// Implementation note (why it's fast): the 6×7 grid is 42 *fixed* buttons,
// created once. Each cell's date is a `computed` of the visible month, and its
// label/class are reactive. So paging months mutates 42 text nodes + classes —
// it never recreates a single DOM node. That's fine-grained reactivity earning
// its keep on a real widget.

import { signal, computed, html } from "../index.js";

// i18n: built-in locales. `weekdays` are indexed by getDay() (0 = Sunday).
// Add more here, or pass custom `months`/`weekdays` arrays as props.
const LOCALES = {
  en: {
    months: ["January", "February", "March", "April", "May", "June",
             "July", "August", "September", "October", "November", "December"],
    weekdays: ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"],
  },
  tr: {
    months: ["Ocak", "Şubat", "Mart", "Nisan", "Mayıs", "Haziran",
             "Temmuz", "Ağustos", "Eylül", "Ekim", "Kasım", "Aralık"],
    weekdays: ["Pa", "Pt", "Sa", "Ça", "Pe", "Cu", "Ct"],
  },
};

const startOfMonth = (d) => new Date(d.getFullYear(), d.getMonth(), 1);
const sameDay = (a, b) =>
  !!a && !!b &&
  a.getFullYear() === b.getFullYear() &&
  a.getMonth() === b.getMonth() &&
  a.getDate() === b.getDate();

export function Calendar(props = {}) {
  const today = new Date();
  const weekStart = props.weekStart ?? 1; // Monday by default
  const selected = props.value ?? signal(null);
  const isDisabled = props.disabled ?? (() => false);

  // i18n: locale ("en" | "tr"), or override months/weekdays arrays directly.
  const loc = LOCALES[props.locale] || LOCALES.en;
  const MONTHS = props.months ?? loc.months;
  const WEEKDAYS = props.weekdays ?? loc.weekdays;

  // range mode: pick a start then an end; a disabled (booked) day clamps the range.
  const range = props.range ?? false;
  const start = props.start ?? signal(null);
  const end = props.end ?? signal(null);

  const initial =
    (selected.peek && selected.peek()) || (start.peek && start.peek()) || today;
  const view = signal(startOfMonth(initial)); // first day of the month on screen

  const prevMonth = () => view.update((v) => new Date(v.getFullYear(), v.getMonth() - 1, 1));
  const nextMonth = () => view.update((v) => new Date(v.getFullYear(), v.getMonth() + 1, 1));

  const nextDay = (d) => new Date(d.getFullYear(), d.getMonth(), d.getDate() + 1);

  const pick = (d) => {
    if (isDisabled(d)) return;            // booked/past days are not selectable
    if (!range) {
      selected.set(d);
      props.onSelect && props.onSelect(d);
      return;
    }
    const s = start(), e = end();
    if (!s || e || d <= s) {
      // begin (or restart) the range
      start.set(d);
      end.set(null);
      return;
    }
    // s is set, no e yet, d > s → set checkout, but clamp at the first booked night
    let chosen = d;
    let cur = nextDay(s);
    while (cur < d) {
      if (isDisabled(cur)) { chosen = cur; break; }   // can't span a booked night
      cur = nextDay(cur);
    }
    end.set(chosen);
    props.onRange && props.onRange(s, chosen);
  };

  const title = () => `${MONTHS[view().getMonth()]} ${view().getFullYear()}`;

  // 42 cells, each a reactive date derived from the visible month.
  const cells = [];
  for (let i = 0; i < 42; i++) {
    const date = computed(() => {
      const v = view();
      const lead = (new Date(v.getFullYear(), v.getMonth(), 1).getDay() - weekStart + 7) % 7;
      return new Date(v.getFullYear(), v.getMonth(), 1 - lead + i);
    });
    cells.push(date);
  }

  const weekdays = WEEKDAYS.slice(weekStart).concat(WEEKDAYS.slice(0, weekStart));

  return html`
    <div class="zap-cal">
      <div class="zap-cal__header">
        <button class="zap-cal__nav" aria-label="Previous month" onclick=${prevMonth}>‹</button>
        <div class="zap-cal__title">${title}</div>
        <button class="zap-cal__nav" aria-label="Next month" onclick=${nextMonth}>›</button>
      </div>
      <div class="zap-cal__grid">
        ${weekdays.map((w) => html`<div class="zap-cal__weekday">${w}</div>`)}
        ${cells.map((date) => {
          const cls = () => {
            const d = date();
            let c = "zap-cal__day";
            if (d.getMonth() !== view().getMonth()) c += " is-muted";
            if (sameDay(d, today)) c += " is-today";
            if (range) {
              const s = start(), e = end();
              if (s && sameDay(d, s)) c += " is-range-start is-selected";
              if (e && sameDay(d, e)) c += " is-range-end is-selected";
              if (s && e && d > s && d < e) c += " is-in-range";
            } else if (sameDay(d, selected())) {
              c += " is-selected";
            }
            if (isDisabled(d)) c += " is-disabled";
            return c;
          };
          return html`<button class=${cls}
                              disabled=${() => isDisabled(date())}
                              onclick=${() => pick(date())}>${() => date().getDate()}</button>`;
        })}
      </div>
    </div>`;
}
