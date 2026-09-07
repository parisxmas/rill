/* Vendored from zap (https://github.com/barisakin/zap), MIT, unmodified.
   The catalogue serves it, so the example stands on its own. */
// Keeping an open panel out of whatever would clip it.
//
// A select or a date picker hangs its panel under its trigger with
// `position: absolute`, which is right until the thing sits inside something
// that scrolls — a data grid's rows, a modal's body, a card with
// `overflow: auto`. Then the panel is cut off at that box's edge, and a
// calendar opened in the last column of a table is simply not there.
//
// `anchor` takes the panel out of that flow: it is placed with
// `position: fixed` at the trigger's place on the screen, flipped above when
// there is no room below, and nudged in when it would hang off the right.
// Scrolling or resizing puts it back where it belongs. Returns the function
// that stops watching — hand it to `onCleanup`.
// Whether anything between here and the document would cut the panel off.
function clipped(el) {
  for (let p = el && el.parentElement; p && p !== document.body; p = p.parentElement) {
    const o = getComputedStyle(p);
    if (o.overflowX !== "visible" || o.overflowY !== "visible") return true;
  }
  return false;
}

export function anchor(panel, trigger) {
  // Nothing to escape, nothing to do: a panel that is not inside anything
  // clipping is left exactly where the stylesheet put it. Taking it out of
  // flow when it did not need to be is how a menu ends up the width of the
  // window — `left: 0; right: 0` means its parent while it is absolute and
  // means the screen once it is fixed.
  if (!clipped(trigger)) return () => {};

  const place = () => {
    if (!panel.isConnected || !trigger || !trigger.isConnected) return;
    const at = trigger.getBoundingClientRect();
    panel.style.position = "fixed";
    panel.style.margin = "0";
    panel.style.right = "auto";
    panel.style.bottom = "auto";
    if (!panel.style.minWidth) panel.style.minWidth = at.width + "px";
    const box = panel.getBoundingClientRect();
    const room = window.innerHeight - at.bottom;
    const above = box.height > room && at.top > box.height;
    panel.style.top = (above ? at.top - box.height : at.bottom) + "px";
    panel.style.left =
      Math.max(8, Math.min(at.left, window.innerWidth - box.width - 8)) + "px";
  };
  // After the panel is in the document, or there is nothing to measure.
  Promise.resolve().then(place);
  const again = () => place();
  addEventListener("scroll", again, true);
  addEventListener("resize", again);
  return () => {
    removeEventListener("scroll", again, true);
    removeEventListener("resize", again);
  };
}
