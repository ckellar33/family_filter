// Auto-filter state: the active filter list (filterSummary/filterEnabled/
// categoryEnabled -- what's actually muting/skipping right now) plus the
// Select Filter tab's own state -- the poster grid and whichever entry's
// detail (categories/cues) is currently open, including the in-detail
// service switcher for titles with more than one variant. Also the Open
// Controls "a filter is available" auto-detect banner's state, since it's
// built on the same library-wide service lookup the switcher uses.
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { session, refreshPlayback } from "$lib/state/session.svelte";
import { parseTime } from "$lib/format";
import type { Cue, FilterEntryDetail, FilterSummary, FilterTile, ServiceOption } from "$lib/types";

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

  // Every service variant of whichever title is open in `detail` below --
  // drives the in-detail "switch service" control once more than one
  // exists (see openTitle: there's no separate picker step, tapping a tile
  // goes straight to a best-guess variant and the switcher handles the
  // rest).
  serviceOptions: [] as ServiceOption[],

  // Select Filter detail -- set once a (title, service) entry is open;
  // `selectedPath` is kept alongside `detail` (not part of the wire type)
  // so toggling a category/cue can re-select the same entry to refresh it.
  detail: null as FilterEntryDetail | null,
  selectedPath: null as string | null,
  detailLoading: false,
  detailError: "",

  // Open Controls' "a filter is available for what's playing" banner --
  // set by checkAvailableForPlayback (see +page.svelte's effect that calls
  // it), null whenever nothing needs enabling.
  availableHint: null as (ServiceOption & { title: string }) | null,
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

// What tapping a poster tile does: no intermediate picker step -- this
// looks up every service variant of `title` (see filter::MediaEntry's doc
// comment for why a title can have more than one) and opens straight into
// a best guess: whichever matches what's actually playing right now, else
// just the first one on record. `serviceOptions` is populated regardless,
// so the detail view's "switch service" control (see
// SelectFilterPage.svelte) is always there to correct the guess.
export async function openTitle(title: string) {
  filterState.detailError = "";
  try {
    const options = await invoke<ServiceOption[]>("list_services_for_title", { title });
    if (options.length === 0) {
      filterState.detailError = `No filter entries found for "${title}".`;
      return;
    }
    filterState.serviceOptions = options;

    const nowPlayingService = currentlyPlayingService(title);
    const autoMatch = nowPlayingService ? options.find((o) => o.service.toLowerCase() === nowPlayingService.toLowerCase()) : undefined;
    const chosen = autoMatch ?? options[0];
    await selectTile(chosen.path, title, chosen.service);
  } catch (e) {
    filterState.detailError = String(e);
  }
}

// The app currently "now playing" `title`'s service name, if that's indeed
// what's on screen right now -- shared by openTitle's auto-pick and
// checkAvailableForPlayback below.
function currentlyPlayingService(title: string): string | null {
  const p = session.playback;
  if (!p?.title || !p.app_name) return null;
  return p.title.trim().toLowerCase() === title.trim().toLowerCase() ? p.app_name : null;
}

// Loads `path` as the active auto-filter list and opens the `(title,
// service)` entry's detail view -- what openTitle's guess resolves to, and
// also how the in-detail "switch service" control (see
// SelectFilterPage.svelte) re-selects a sibling variant. Also refreshes
// `filterSummary`/`categoryEnabled` the same way `checkSavedFilter` does,
// since this *is* a filter-file load.
export async function selectTile(path: string, title: string, service: string) {
  filterState.detailLoading = true;
  filterState.detailError = "";
  try {
    const detail = await invoke<FilterEntryDetail>("select_filter_tile", { path, title, service });
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
  filterState.serviceOptions = [];
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
    await invoke("set_filter_cue_enabled", {
      title: filterState.detail.title,
      service: filterState.detail.service,
      index: cue.index,
      enabled: !cue.enabled,
    });
    await refreshDetail();
    await refreshPlayback();
  } catch (e) {
    filterState.detailError = String(e);
  }
}

// Retimes one cue in the open detail's entry -- the Filters tab's own cue
// editor (see SelectFilterPage.svelte's CueEditorSheet), as opposed to
// creation.svelte.ts's updateCueTime, which only ever edits a recording
// draft and needs something currently playing to know which entry to target.
// This acts on whatever (title, service) is already open in `detail`, so it
// works regardless of what's on screen right now, and persists straight to
// the filter file on the backend (see control::update_filter_cue) rather
// than just this session's enabled/disabled overrides.
export async function updateDetailCueTime(cue: Cue, field: "start" | "end", text: string) {
  if (!filterState.detail) return;
  const seconds = parseTime(text);
  if (seconds == null) {
    filterState.detailError = `"${text}" isn't a valid m:ss time`;
    return;
  }
  filterState.detailError = "";
  try {
    const start = field === "start" ? seconds : cue.start;
    const end = field === "end" ? seconds : cue.end;
    await invoke("update_filter_cue", { title: filterState.detail.title, service: filterState.detail.service, index: cue.index, start, end });
    await refreshDetail();
    await refreshPlayback();
  } catch (e) {
    filterState.detailError = String(e);
  }
}

// Removes a cue outright from the open detail's entry -- the Filters-tab
// counterpart to creation.svelte.ts's deleteCue, same distinction as
// updateDetailCueTime above.
export async function deleteDetailCue(cue: Cue) {
  if (!filterState.detail) return;
  filterState.detailError = "";
  try {
    await invoke("delete_filter_cue", { title: filterState.detail.title, service: filterState.detail.service, index: cue.index });
    await refreshDetail();
    await refreshPlayback();
  } catch (e) {
    filterState.detailError = String(e);
  }
}

// Re-fetches the open detail view from the backend -- cheap even though it
// re-selects the whole tile (poster lookups are disk-cached by then), used
// after every toggle so the tree reflects the new enabled state immediately
// rather than waiting on a poll.
async function refreshDetail() {
  const path = filterState.selectedPath;
  const detail = filterState.detail;
  if (!path || !detail) return;
  try {
    filterState.detail = await invoke<FilterEntryDetail>("select_filter_tile", { path, title: detail.title, service: detail.service });
  } catch (e) {
    filterState.detailError = String(e);
  }
}

// Checks whether a filter is available for whatever's playing right now but
// not currently enabled -- called from +page.svelte whenever the playing
// title/app changes, powering Open Controls' "a filter is available, tap to
// enable" banner. Deliberately only ever matches the *exact* service that's
// playing (never a same-title entry tagged for a different service) --
// suggesting a filter whose cue times were recorded against a different
// platform's cut risks muting/skipping at the wrong moments, which is worse
// than not suggesting one at all.
export async function checkAvailableForPlayback() {
  const p = session.playback;
  if (!p?.title || !p.app_name) {
    filterState.availableHint = null;
    return;
  }
  // Already matched and turned on -- nothing to nag about.
  if (p.filter_match && filterState.filterEnabled) {
    filterState.availableHint = null;
    return;
  }
  try {
    const options = await invoke<ServiceOption[]>("list_services_for_title", { title: p.title });
    const exact = options.find((o) => o.service.toLowerCase() === p.app_name!.toLowerCase());
    filterState.availableHint = exact ? { ...exact, title: p.title } : null;
  } catch {
    // Best-effort -- a failed lookup just means no banner, not an error
    // worth surfacing over Open Controls' now-playing card.
    filterState.availableHint = null;
  }
}
