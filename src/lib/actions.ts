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

// The iOS "drag in from the left edge to go back" gesture -- a second way
// to trigger whatever `onBack` already does (+page.svelte's `goBack`,
// wired to both the NavBar's `‹ Back` button and, via its own
// closeDetail() branch, SelectFilterPage's `‹ All titles` link), not a
// replacement for either -- both stay exactly as they were.
//
// `node` is +page.svelte's `.edge-swipe-zone`: a thin invisible strip
// nested inside `.canvas` (see app.css) and only ever rendered while
// there's actually somewhere to go back to, so it never exists to eat a
// touch on a screen with nothing behind it. It's deliberately confined to
// `.content`'s own height rather than the navbar/tabbar's -- see the
// z-index comment on `.edge-swipe-zone` -- so it can't shadow the back
// button, the app mark, or a tab.
//
// Same live-follow/retract-aware physics as swipeDownToClose above,
// turned sideways: `.content` -- deliberately not `.canvas`, so the
// sticky navbar and tab bar stay put and only the content between them
// moves -- translates 1:1 with the pointer while it's down, judged
// against the deepest point the drag reached rather than raw total
// travel (so dragging past COMMIT_FRACTION and then pulling back toward
// the edge -- changing your mind mid-gesture -- cancels the back
// navigation instead of committing to it). Past the threshold, `.content`
// keeps sliding until it's fully offscreen and `onBack` only fires once
// that slide's `transitionend` lands; short of it, `.content` springs
// back to translateX(0). Either way the transform is reset the instant
// the settle finishes -- committed, `.content` is offscreen and invisible
// when `onBack` swaps in whatever screen it's going back to, so the reset
// itself is imperceptible; cancelled, there's nothing left to reset.
export function edgeSwipeBack(node: HTMLElement, onBack: () => void) {
  const EDGE_WIDTH = 20; // px from the true left edge a drag has to start within to count
  const COMMIT_FRACTION = 0.3; // fraction of the screen's width to drag before it commits to going back
  const RETRACT_TOLERANCE = 16; // px of pull-back from the deepest point that still counts as "still committing", not a change of mind

  const canvas = node.closest<HTMLElement>(".canvas");
  const content = canvas?.querySelector<HTMLElement>(":scope > .content") ?? null;

  let startX: number | null = null;
  let maxDelta = 0;
  let width = 0;

  function setTransform(x: number) {
    if (content) content.style.transform = x > 0 ? `translateX(${x}px)` : "";
  }

  function onPointerDown(e: PointerEvent) {
    if (!content) return;
    const rect = node.getBoundingClientRect();
    if (e.clientX - rect.left > EDGE_WIDTH) return; // only a drag starting at the true edge counts
    startX = e.clientX;
    maxDelta = 0;
    width = content.clientWidth;
    content.classList.add("edge-dragging");
    node.setPointerCapture(e.pointerId);
  }

  function onPointerMove(e: PointerEvent) {
    if (startX == null || !content) return;
    // Only rightward travel moves it -- dragging back toward the edge
    // meets the same resistance a resting screen gives you (none; it
    // just doesn't budge past home) instead of tucking it under the edge.
    const delta = Math.max(0, e.clientX - startX);
    if (delta > 0 && maxDelta === 0) content.classList.add("edge-active"); // first real movement -- now it's a drag, not a stray tap
    maxDelta = Math.max(maxDelta, delta);
    setTransform(delta);
  }

  function settle(committing: boolean) {
    if (!content) return;
    content.classList.remove("edge-dragging"); // let .content's own transition take over for the settle
    const onTransitionEnd = (e: TransitionEvent) => {
      if (e.propertyName !== "transform") return;
      content.removeEventListener("transitionend", onTransitionEnd);
      content.classList.remove("edge-active");
      if (committing) {
        onBack();
        content.classList.add("edge-dragging"); // reset instantly, no transition -- see comment above
        setTransform(0);
        requestAnimationFrame(() => content.classList.remove("edge-dragging"));
      }
    };
    content.addEventListener("transitionend", onTransitionEnd);
    setTransform(committing ? width + 60 : 0);
  }

  function finishDrag(delta: number) {
    if (maxDelta === 0) {
      // No real movement -- a stray tap at the edge, not a drag. Nothing
      // was ever displaced, so there's nothing to animate back from.
      content?.classList.remove("edge-dragging");
      return;
    }
    const retracting = maxDelta - delta > RETRACT_TOLERANCE;
    settle(delta > width * COMMIT_FRACTION && !retracting);
  }

  function onPointerUp(e: PointerEvent) {
    if (startX == null) return;
    const delta = e.clientX - startX;
    startX = null;
    finishDrag(delta);
  }

  function onPointerCancel() {
    if (startX == null) return;
    startX = null;
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
      // In case this unmounts mid-drag (the screen it'd go back from
      // disappearing out from under it) -- don't leave `.content` stuck
      // half-displaced with no listener left to settle it.
      content?.classList.remove("edge-dragging", "edge-active");
      if (content) content.style.transform = "";
    },
  };
}
