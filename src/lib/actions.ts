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

// Lets a sheet's grabber handle (the small pill at its top -- CueEditorSheet
// and CueLabelSheet both have one) be swiped down to dismiss it, the "drag
// the visible handle to close" gesture a native bottom sheet gives you. The
// grabber was purely decorative until this existed -- tapping the backdrop
// was the only way to close either sheet.
//
// Deliberately scoped to the grabber itself rather than the whole sheet
// body: `.sheet` can scroll internally (its own `overflow:auto`, for a
// sheet taller than 88vh) and is full of buttons/inputs, so a whole-body
// swipe-to-dismiss would have to carefully tell "scrolling content" and
// "tapping a button" apart from "dragging to close". A dedicated handle
// sidesteps all of that -- there's nothing else on it to conflict with.
//
// No live drag-follows-your-finger animation, matching how these sheets
// already just appear/disappear instantly on open/close (tapping the
// backdrop has never animated either) -- this only detects the gesture and
// calls `onDismiss` once it crosses the threshold, the same instant close
// every other dismissal path already gives.
export function swipeDownToClose(node: HTMLElement, onDismiss: () => void) {
  const THRESHOLD = 24; // px of downward travel before it counts as a swipe, not just an imprecise tap

  let startY: number | null = null;

  function onPointerDown(e: PointerEvent) {
    startY = e.clientY;
  }

  function onPointerUp(e: PointerEvent) {
    if (startY == null) return;
    const delta = e.clientY - startY;
    startY = null;
    if (delta > THRESHOLD) onDismiss();
  }

  function onPointerCancel() {
    startY = null;
  }

  node.addEventListener("pointerdown", onPointerDown);
  node.addEventListener("pointerup", onPointerUp);
  node.addEventListener("pointercancel", onPointerCancel);

  return {
    destroy() {
      node.removeEventListener("pointerdown", onPointerDown);
      node.removeEventListener("pointerup", onPointerUp);
      node.removeEventListener("pointercancel", onPointerCancel);
    },
  };
}
