// Filter *creation* mode: recording cue timestamps live from playback,
// as opposed to filter.svelte.ts's auto-filter state, which describes
// applying an already-authored file. Deliberately separate even though the
// shapes overlap, since a draft cue has no `enabled` (that's an
// auto-filter-only concept) -- the draft being authored here is never the
// list actively muting/skipping mid-movie unless useDraftAsActiveFilter()
// explicitly arms it.
import { invoke } from "@tauri-apps/api/core";
import { open, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { session } from "$lib/state/session.svelte";
import { filterState } from "$lib/state/filter.svelte";
import { parseTime } from "$lib/format";
import type { CategoryDef, CategoryKind, CreationCue, DraftSummary, FilterSummary } from "$lib/types";

export const DEFAULT_CATEGORIES: CategoryDef[] = [
  { name: "language", kind: "mute" },
  { name: "violence", kind: "skip" },
  { name: "gore", kind: "skip" },
  { name: "nudity", kind: "skip" },
  { name: "peril", kind: "skip" },
];

export const creationState = $state({
  // "idle" until a draft is started/opened, then "recording" while category
  // buttons + the cue table are shown.
  stage: "idle" as "idle" | "recording",
  draft: null as DraftSummary | null,
  categories: [...DEFAULT_CATEGORIES] as CategoryDef[],
  cues: [] as CreationCue[],
  pendingSkipCategory: null as string | null,
  busy: false,
  error: "",
  newCategoryName: "",
  newCategoryKind: "skip" as CategoryKind,

  // Service tags (see filter::MediaEntry::services) for whatever's
  // currently playing -- auto-tagged backend-side from the playing app on a
  // title's first cue, shown here as an editable chip list.
  services: [] as string[],
  newServiceName: "",
});

// Backend's `start_control_session` rebuilds ControlState from scratch --
// including creation-mode's draft -- so the frontend's view of it needs
// resetting alongside filter.svelte.ts's checkSavedFilter() whenever that
// happens (see +page.svelte).
export function resetCreation() {
  creationState.stage = "idle";
  creationState.draft = null;
  creationState.cues = [];
  creationState.services = [];
  creationState.pendingSkipCategory = null;
  creationState.error = "";
}

export async function pickNewDraft() {
  creationState.error = "";
  try {
    const path = await saveDialog({ filters: [{ name: "Filter list", extensions: ["json"] }], defaultPath: "filter.json" });
    if (!path) return; // user cancelled
    creationState.draft = await invoke<DraftSummary>("creation_new_draft", { path });
    creationState.stage = "recording";
    creationState.cues = [];
    creationState.services = [];
  } catch (e) {
    creationState.error = String(e);
  }
}

export async function pickExistingDraft() {
  creationState.error = "";
  try {
    const path = await open({ multiple: false, filters: [{ name: "Filter list", extensions: ["json"] }] });
    if (!path || Array.isArray(path)) return; // user cancelled
    creationState.draft = await invoke<DraftSummary>("creation_open_draft", { path });
    creationState.stage = "recording";
    await refreshCreationCues();
    await refreshServices();
  } catch (e) {
    creationState.error = String(e);
  }
}

// Re-fetches the draft's cues for whatever's currently playing -- called
// after every mutation, and reactively (see CreateFilterPage.svelte)
// whenever the title changes while recording, so the table always reflects
// the title actually on screen.
export async function refreshCreationCues() {
  if (!session.playback?.title) {
    creationState.cues = [];
    return;
  }
  try {
    creationState.cues = await invoke<CreationCue[]>("creation_list_cues", { title: session.playback.title });
  } catch (e) {
    creationState.error = String(e);
  }
}

// Same as refreshCreationCues, but for the title's service tags -- kept
// separate since it's a different backend command, but always called
// alongside it (see CreateFilterPage.svelte's title-change effect) so the
// chip list and cue table never fall out of sync with each other.
export async function refreshServices() {
  if (!session.playback?.title) {
    creationState.services = [];
    return;
  }
  try {
    creationState.services = await invoke<string[]>("creation_list_services", { title: session.playback.title });
  } catch (e) {
    creationState.error = String(e);
  }
}

export async function addService() {
  const service = creationState.newServiceName.trim();
  if (!service || !session.playback?.title) return;
  creationState.error = "";
  try {
    await invoke("creation_add_service", { title: session.playback.title, service });
    creationState.newServiceName = "";
    await refreshServices();
  } catch (e) {
    creationState.error = String(e);
  }
}

export async function removeService(service: string) {
  if (!session.playback?.title) return;
  creationState.error = "";
  try {
    await invoke("creation_remove_service", { title: session.playback.title, service });
    await refreshServices();
  } catch (e) {
    creationState.error = String(e);
  }
}

export async function markMute(category: string) {
  creationState.busy = true;
  creationState.error = "";
  try {
    await invoke("creation_mark_mute", { category });
    await refreshCreationCues();
    // A mark can auto-tag the title's very first service -- refresh so the
    // chip appears without waiting for the next title-change effect.
    await refreshServices();
  } catch (e) {
    creationState.error = String(e);
  } finally {
    creationState.busy = false;
  }
}

// First press on a skip-category button starts its mark; the second press
// on that *same* button ends it. Other skip buttons are disabled in the
// markup while pendingSkipCategory is set, so the "already recording a
// different category" branch below is a safety net, not the normal path.
export async function toggleSkipMark(category: string) {
  creationState.busy = true;
  creationState.error = "";
  try {
    if (creationState.pendingSkipCategory === category) {
      await invoke("creation_end_skip_mark");
      creationState.pendingSkipCategory = null;
      await refreshCreationCues();
      await refreshServices();
    } else if (creationState.pendingSkipCategory === null) {
      await invoke("creation_start_skip_mark", { category });
      creationState.pendingSkipCategory = category;
    }
  } catch (e) {
    creationState.error = String(e);
  } finally {
    creationState.busy = false;
  }
}

export async function cancelSkipMark() {
  creationState.busy = true;
  creationState.error = "";
  try {
    await invoke("creation_cancel_skip_mark");
    creationState.pendingSkipCategory = null;
  } catch (e) {
    creationState.error = String(e);
  } finally {
    creationState.busy = false;
  }
}

export async function updateCueTime(cue: CreationCue, field: "start" | "end", text: string) {
  const seconds = parseTime(text);
  if (seconds == null) {
    creationState.error = `"${text}" isn't a valid m:ss time`;
    return;
  }
  if (!session.playback?.title) return;
  creationState.error = "";
  try {
    const start = field === "start" ? seconds : cue.start;
    const end = field === "end" ? seconds : cue.end;
    await invoke("creation_update_cue", { title: session.playback.title, index: cue.index, start, end });
    await refreshCreationCues();
  } catch (e) {
    creationState.error = String(e);
  }
}

export async function deleteCue(cue: CreationCue) {
  if (!session.playback?.title) return;
  creationState.error = "";
  try {
    await invoke("creation_delete_cue", { title: session.playback.title, index: cue.index });
    await refreshCreationCues();
  } catch (e) {
    creationState.error = String(e);
  }
}

export function addCustomCategory() {
  const name = creationState.newCategoryName.trim();
  if (!name || creationState.categories.some((c) => c.name === name)) return;
  creationState.categories = [...creationState.categories, { name, kind: creationState.newCategoryKind }];
  creationState.newCategoryName = "";
}

// Reloads the draft's own file into the (separate) auto-filter list, so
// cues just recorded can be tried out live without going through the
// Select Filter grid.
export async function useDraftAsActiveFilter() {
  if (!creationState.draft) return;
  filterState.filterBusy = true;
  filterState.filterError = "";
  try {
    const summary = await invoke<FilterSummary>("load_filter_file", { path: creationState.draft.path });
    filterState.filterSummary = summary;
    filterState.filterEnabled = false;
    filterState.categoryEnabled = Object.fromEntries(summary.categories.map((c) => [c, true]));
  } catch (e) {
    filterState.filterError = String(e);
  } finally {
    filterState.filterBusy = false;
  }
}
