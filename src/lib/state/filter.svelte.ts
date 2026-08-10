// Auto-filter state: the active filter list (filterSummary/filterEnabled/
// categoryEnabled -- what's actually muting/skipping right now) plus the
// Select Filter tab's own state -- the poster grid and whichever tile's
// detail (categories/cues/streaming badges) is currently open.
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { refreshPlayback } from "$lib/state/session.svelte";
import type { Cue, FilterEntryDetail, FilterSummary, FilterTile } from "$lib/types";

export const filterState = $state({
  // Auto-filter mode: a loaded cue file (filterSummary), the master on/off
  // toggle, and per-category on/off state keyed by category name -- absent
  // from the map (or true) means enabled, matching the backend's
  // disabled_categories-is-the-exception-list convention.
  filterSummary: null as FilterSummary | null,
  filterEnabled: false,
  categoryEnabled: {} as Record<string, boolean>,
  filterBusy: false,
  filterError: "",

  // Select Filter grid.
  tiles: [] as FilterTile[],
  tilesLoading: false,
  tilesError: "",

  // Select Filter detail -- set once a tile is tapped; `selectedPath` is
  // kept alongside `detail` (not part of the wire type) so toggling a
  // category/cue can re-select the same tile to refresh it.
  detail: null as FilterEntryDetail | null,
  selectedPath: null as string | null,
  detailLoading: false,
  detailError: "",
});

// Tries to reload whatever filter file was last picked (persisted
// backend-side). Leaves the mode off either way -- loading a list must
// never silently start auto-muting/skipping. Called right after a control
// session opens, same as the original single-page version did.
export async function checkSavedFilter() {
  filterState.filterSummary = null;
  filterState.filterEnabled = false;
  filterState.categoryEnabled = {};
  filterState.filterError = "";
  try {
    const found = await invoke<FilterSummary | null>("check_saved_filter_file");
    if (found) {
      filterState.filterSummary = found;
      filterState.categoryEnabled = Object.fromEntries(found.categories.map((c) => [c, true]));
    }
  } catch (e) {
    filterState.filterError = String(e);
  }
}

export async function toggleFilterEnabled() {
  const next = !filterState.filterEnabled;
  filterState.filterBusy = true;
  filterState.filterError = "";
  try {
    await invoke("set_filter_enabled", { enabled: next });
    filterState.filterEnabled = next;
  } catch (e) {
    filterState.filterError = String(e);
  } finally {
    filterState.filterBusy = false;
  }
}

// Lets the user pick one or more filter files (native multi-select picker)
// to add to the library -- they show up as grid tiles, but aren't made the
// active list just by being added (see control::add_filter_files).
export async function addFilterFiles() {
  filterState.tilesError = "";
  try {
    const paths = await open({ multiple: true, filters: [{ name: "Filter list", extensions: ["json"] }] });
    if (!paths || paths.length === 0) return; // user cancelled
    await invoke("add_filter_files", { paths });
    await loadTiles();
  } catch (e) {
    filterState.tilesError = String(e);
  }
}

// Lets the user point at a whole folder of filter files instead of adding
// them one at a time -- every top-level .json file that parses as a filter
// list gets registered (see control::add_filter_directory).
export async function addFilterDirectory() {
  filterState.tilesError = "";
  try {
    const path = await open({ directory: true, multiple: false });
    if (!path || Array.isArray(path)) return; // user cancelled
    await invoke("add_filter_directory", { path });
    await loadTiles();
  } catch (e) {
    filterState.tilesError = String(e);
  }
}

// Every title across every known filter file, for the poster grid --
// backed by control::list_filter_tiles, which also resolves each title's
// poster (cached TMDB lookup, so this is cheap after the first run).
export async function loadTiles() {
  filterState.tilesLoading = true;
  filterState.tilesError = "";
  try {
    filterState.tiles = await invoke<FilterTile[]>("list_filter_tiles");
  } catch (e) {
    filterState.tilesError = String(e);
  } finally {
    filterState.tilesLoading = false;
  }
}

// Loads `path` as the active auto-filter list and opens `title`'s detail
// view (categories/cues/streaming badges) -- what tapping a poster tile
// does. Also refreshes `filterSummary`/`categoryEnabled` the same way
// `checkSavedFilter` does, since this *is* a filter-file load, just reached
// from the grid instead of a file picker.
export async function selectTile(path: string, title: string) {
  filterState.detailLoading = true;
  filterState.detailError = "";
  try {
    const detail = await invoke<FilterEntryDetail>("select_filter_tile", { path, title });
    filterState.detail = detail;
    filterState.selectedPath = path;
    filterState.filterEnabled = false;
    filterState.categoryEnabled = Object.fromEntries(detail.categories.map((c) => [c, true]));
    // FilterSummary isn't part of select_filter_tile's response -- re-derive
    // just enough of it (path/category list) for the rest of the app (e.g.
    // Open Controls' "no filter list loaded" check) without a second
    // round trip; media_count isn't shown anywhere that matters here, so 0
    // is fine as a placeholder.
    filterState.filterSummary = { path, media_count: 0, categories: detail.categories };
    await refreshPlayback();
  } catch (e) {
    filterState.detailError = String(e);
  } finally {
    filterState.detailLoading = false;
  }
}

export function closeDetail() {
  filterState.detail = null;
  filterState.selectedPath = null;
  filterState.detailError = "";
}

export async function toggleDetailCategory(category: string) {
  const next = !filterState.categoryEnabled[category];
  filterState.detailError = "";
  try {
    await invoke("set_filter_category_enabled", { category, enabled: next });
    filterState.categoryEnabled = { ...filterState.categoryEnabled, [category]: next };
    await refreshDetail();
    await refreshPlayback();
  } catch (e) {
    filterState.detailError = String(e);
  }
}

// Unlike categoryEnabled, per-cue enabled state isn't tracked separately
// client-side -- `detail.cues[i].enabled` already comes back fresh from the
// backend on every refresh, so that's the single source of truth; toggling
// just tells the backend and re-fetches.
export async function toggleDetailCue(cue: Cue) {
  if (!filterState.detail) return;
  filterState.detailError = "";
  try {
    await invoke("set_filter_cue_enabled", { title: filterState.detail.title, index: cue.index, enabled: !cue.enabled });
    await refreshDetail();
    await refreshPlayback();
  } catch (e) {
    filterState.detailError = String(e);
  }
}

// Re-fetches the open detail view from the backend -- cheap even though it
// re-selects the whole tile (poster/streaming-provider lookups are disk-
// cached by then), used after every toggle so the tree reflects the new
// enabled state immediately rather than waiting on a poll.
async function refreshDetail() {
  const path = filterState.selectedPath;
  const title = filterState.detail?.title;
  if (!path || !title) return;
  try {
    filterState.detail = await invoke<FilterEntryDetail>("select_filter_tile", { path, title });
  } catch (e) {
    filterState.detailError = String(e);
  }
}
