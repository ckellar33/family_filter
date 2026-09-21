// Shared Svelte actions.

// Re-homes a `position: fixed` overlay (a sheet/modal backdrop) onto
// document.body instead of wherever it was declared in the component tree.
//
// Why this exists: CueEditorSheet's backdrop is `position: fixed`, but it's
// declared inside CreateFilterPage/SelectFilterPage, which render inside
// `.content` -- a div with `-webkit-overflow-scrolling: touch` (app.css) for
// momentum scrolling. In WKWebView (and historically Safari), a `position:
// fixed` descendant of a `-webkit-overflow-scrolling: touch` ancestor is
// clipped to that ancestor's box instead of the true viewport, so the sheet
// got cut off wherever `.content` happened to end -- hiding its Save/Delete
// row with no way to scroll to it. Moving the node to `document.body` on
// mount takes it out of `.content`'s subtree entirely, so `position: fixed`
// means the viewport again.
export function portal(node: HTMLElement) {
  document.body.appendChild(node);

  return {
    destroy() {
      node.remove();
    },
  };
}

// Lets a sheet's top drag region -- the grabber pill plus whatever inert
// header row sits right under it (see each sheet's `.sheet-drag-region`
// wrapper in its markup) -- be swiped down to dismiss it, the "drag
// anywhere across the top chrome to close" gesture a native iOS sheet
// gives you (not just its handle). That region was purely decorative
// until this existed -- tapping the backdrop was the only way to close
// any of these sheets.
//
// Deliberately scoped to that top region rather than the whole sheet
// body: `.sheet` can scroll internally (its own `overflow:auto`, for a
// sheet taller than 88vh) and is full of buttons/inputs, so a whole-body
// swipe-to-dismiss would have to carefully tell "scrolling content" and
// "tapping a button" apart from "dragging to close". The drag region is
// deliberately kept to inert content (a title, a summary row -- never a
// button or input) so it sidesteps all of that -- there's nothing on it
// to conflict with.
//
// Tracks the drag live, iOS-style: the sheet (found via this node's
// closest `.sheet` ancestor -- see app.css) translates 1:1 with the
// pointer while it's down, with `.sheet`'s own CSS transition suspended
// (the `.dragging` class) so there's no lag chasing your finger. On
// release it goes one of two ways, both handled by that same CSS
// transition kicking back in: short of THRESHOLD springs back up to
// translateY(0); past it, the sheet keeps sliding down (to well past any
// sheet's height) and `onDismiss` only fires once that slide's
// `transitionend` lands, so the sheet is actually offscreen before the
// caller unmounts it rather than just vanishing mid-slide.
//
// Whether a release dismisses isn't just "did total travel clear
// THRESHOLD" -- it's judged against the deepest point the drag reached
// (`maxDelta`). Dragging past THRESHOLD and letting go while still headed
// down commits to closing; dragging back up first -- changing your mind
// mid-gesture, the way a native sheet lets you -- cancels it and springs
// back open even though you did, at some point, clear the threshold.
export function swipeDownToClose(node: HTMLElement, onDismiss: () => void) {
  const THRESHOLD = 24; // px of downward travel before it counts as a swipe, not just an imprecise tap
  const DISMISS_TRAVEL = 600; // px to slide down on a committed dismiss -- more than any sheet is tall
  const RETRACT_TOLERANCE = 8; // px of pull-back from the deepest point that still counts as "still headed down", not a change of mind

  const sheet = node.closest<HTMLElement>(".sheet");

  let startY: number | null = null;
  let maxDelta = 0;

  function setTransform(y: number) {
    if (sheet) sheet.style.transform = y > 0 ? `translateY(${y}px)` : "";
  }

  function onPointerDown(e: PointerEvent) {
    startY = e.clientY;
    maxDelta = 0;
    sheet?.classList.add("dragging");
    node.setPointerCapture(e.pointerId);
  }

  function onPointerMove(e: PointerEvent) {
    if (startY == null) return;
    // Only downward travel moves it -- dragging up meets the same
    // resistance a resting bottom sheet gives you (none; it just doesn't
    // budge) instead of floating the handle above its home position.
    const delta = Math.max(0, e.clientY - startY);
    maxDelta = Math.max(maxDelta, delta);
    setTransform(delta);
  }

  function finishDrag(delta: number) {
    sheet?.classList.remove("dragging");
    const retracting = maxDelta - delta > RETRACT_TOLERANCE;
    if (delta > THRESHOLD && !retracting && sheet) {
      const onTransitionEnd = (e: TransitionEvent) => {
        if (e.propertyName !== "transform") return;
        sheet.removeEventListener("transitionend", onTransitionEnd);
        onDismiss();
      };
      sheet.addEventListener("transitionend", onTransitionEnd);
      setTransform(DISMISS_TRAVEL);
    } else if (delta > THRESHOLD && !retracting) {
      onDismiss(); // no `.sheet` ancestor found -- fall back to the old instant close
    } else {
      setTransform(0); // short of the threshold, or pulled back up: spring back home
    }
  }

  function onPointerUp(e: PointerEvent) {
    if (startY == null) return;
    const delta = e.clientY - startY;
    startY = null;
    finishDrag(delta);
  }

  function onPointerCancel() {
    if (startY == null) return;
    startY = null;
    finishDrag(0);
  }

  node.addEventListener("pointerdown", onPointerDown);
  node.addEventListener("pointermove", onPointerMove);
  node.addEventListener("pointerup", onPointerUp);
  node.addEventListener("pointercancel", onPointerCancel);

  return {
    destroy() {
      node.removeEventListener("pointerdown", onPointerDown);
      node.removeEventListener("pointermove", onPointerMove);
      node.removeEventListener("pointerup", onPointerUp);
      node.removeEventListener("pointercancel", onPointerCancel);
    },
  };
}
