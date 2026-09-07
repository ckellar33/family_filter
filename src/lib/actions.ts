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
