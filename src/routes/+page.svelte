<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { open, save as saveDialog } from "@tauri-apps/plugin-dialog";

  interface Device {
    host: string;
    port: number;
  }

  interface SavedPairingInfo {
    host: string;
    port: number;
    has_mrp: boolean;
    has_airplay: boolean;
  }

  interface ControlInfo {
    has_live: boolean;
  }

  interface Cue {
    // Position within the matched entry's cue list -- what
    // set_filter_cue_enabled expects back to identify this cue.
    index: number;
    start: number;
    end: number;
    action: "mute" | "skip";
    category: string;
    // Whether this cue would actually fire right now: both its category
    // and this specific cue are enabled. False either way looks the same
    // to the frontend, so there's no need to track the two separately here.
    enabled: boolean;
  }

  interface PlaybackStatus {
    title: string | null;
    position: number | null;
    duration: number | null;
    playback_state: string;
    // Populated whenever a filter list is loaded and its title matches --
    // regardless of whether auto-filter mode is actually turned on -- so
    // the schedule is visible as a preview even while it's off.
    filter_match: string | null;
    filter_cues: Cue[];
    // Only ever set while auto-filter mode is on (nothing is actually
    // applied while it's off).
    filter_action: string | null;
    filter_category: string | null;
  }

  interface FilterSummary {
    path: string;
    media_count: number;
    categories: string[];
  }

  // Filter *creation* mode -- recording cue timestamps live from playback,
  // as opposed to the auto-filter types above, which describe applying an
  // already-authored file. Deliberately separate types even though the
  // shapes overlap, since a draft cue has no `enabled` (that's an
  // auto-filter-only concept).
  interface DraftSummary {
    path: string;
    media_count: number;
  }

  interface CreationCue {
    index: number;
    start: number;
    end: number;
    action: "mute" | "skip";
    category: string;
  }

  type CategoryKind = "mute" | "skip";
  interface CategoryDef {
    name: string;
    kind: CategoryKind;
  }

  type Protocol = "companion" | "mrp" | "airplay";
  type Step = Protocol | "save" | "done";

  // Companion is required (it's what unlocks mute/skip control); MRP and
  // AirPlay are each their own optional pairing ceremony against their own
  // discovered device, needed only for live playback position. Mirrors
  // libs/appletv-cli's pair_flow().
  const STEPS: Step[] = ["companion", "mrp", "airplay", "save", "done"];
  const PROTOCOL_LABEL: Record<Protocol, string> = {
    companion: "Companion",
    mrp: "MRP",
    airplay: "AirPlay",
  };

  // Top-level page: "checking" while we ask the backend for a saved
  // pairing.store on mount, "saved" if one was found (offer to verify it
  // instead of re-pairing from scratch), "wizard" for the discover-and-pair
  // flow -- either because there was nothing saved, or the user chose to
  // pair a different device anyway -- and "control" once a control session
  // is open (mute/unmute, skip, now playing).
  type Page = "checking" | "saved" | "wizard" | "control";
  let page = $state<Page>("checking");
  let savedPairing = $state<SavedPairingInfo | null>(null);
  let verifying = $state(false);
  let verifyResult = $state<"ok" | "failed" | null>(null);
  let verifyError = $state("");

  let hasLive = $state(false);
  let playback = $state<PlaybackStatus | null>(null);
  let controlBusy = $state(false);
  let controlError = $state("");

  // Auto-filter mode: a loaded cue file (filterSummary), the master on/off
  // toggle, and per-category on/off state keyed by category name -- absent
  // from the map (or true) means enabled, matching the backend's
  // disabled_categories-is-the-exception-list convention.
  let filterSummary = $state<FilterSummary | null>(null);
  let filterEnabled = $state(false);
  let categoryEnabled = $state<Record<string, boolean>>({});
  let filterBusy = $state(false);
  let filterError = $state("");

  // Filter creation mode: "idle" until a draft is started/opened, then
  // "recording" while category buttons + the cue table are shown. Separate
  // from filterSummary/filterEnabled above on purpose -- the draft being
  // authored here is never the list actively muting/skipping mid-movie
  // unless the user explicitly arms it via useDraftAsActiveFilter().
  const DEFAULT_CATEGORIES: CategoryDef[] = [
    { name: "language", kind: "mute" },
    { name: "violence", kind: "skip" },
    { name: "gore", kind: "skip" },
    { name: "nudity", kind: "skip" },
    { name: "peril", kind: "skip" },
  ];
  let creationStage = $state<"idle" | "recording">("idle");
  let draft = $state<DraftSummary | null>(null);
  let categories = $state<CategoryDef[]>(DEFAULT_CATEGORIES);
  let creationCues = $state<CreationCue[]>([]);
  let pendingSkipCategory = $state<string | null>(null);
  let creationBusy = $state(false);
  let creationError = $state("");
  let newCategoryName = $state("");
  let newCategoryKind = $state<CategoryKind>("skip");

  // The single soonest cue that hasn't fully passed yet and would actually
  // fire (category + individual toggle both on) -- what shows up next to
  // the playback position, rather than the whole schedule. filter_cues
  // already arrives sorted by start, so the first match is the soonest one.
  let nextCue = $derived.by(() => {
    const p = playback;
    if (!p) return null;
    return p.filter_cues.find((c) => c.enabled && (p.position == null || c.end > p.position)) ?? null;
  });

  // The currently-matched title's cues, grouped by category, for the
  // categories-as-a-tree view -- each category is expandable to show (and
  // individually toggle) just its own cues.
  let cuesByCategory = $derived.by(() => {
    const grouped: Record<string, Cue[]> = {};
    for (const cue of playback?.filter_cues ?? []) {
      (grouped[cue.category] ??= []).push(cue);
    }
    return grouped;
  });

  // Which categories are expanded in the tree. Absent means "default" --
  // expanded whenever there's something to show, so the tree opens up
  // ready-to-read rather than making you click through every category
  // after loading a file or matching a new title.
  let expandedCategories = $state<Record<string, boolean>>({});

  function toggleExpanded(category: string) {
    expandedCategories = { ...expandedCategories, [category]: !(expandedCategories[category] ?? true) };
  }

  let step = $state<Step>("companion");
  let devices = $state<Device[]>([]);
  let scanning = $state(false);
  let pairing = $state(false);
  let error = $state("");

  // Set once the backend's `pin-requested` event fires for the protocol
  // currently being paired -- i.e. the on-screen code is now showing on the
  // Apple TV and the backend is awaiting `submit_pin`.
  let awaitingPinFor = $state<Protocol | null>(null);
  let pin = $state("");

  function isProtocol(s: Step): s is Protocol {
    return s === "companion" || s === "mrp" || s === "airplay";
  }

  // Runs once on mount (no reactive dependencies) to decide the starting
  // page: straight into the wizard if there's nothing saved yet, or the
  // saved-pairing screen if libs/appletv-cli (or an earlier run of this
  // app) already produced a pairing.store.
  $effect(() => {
    checkSaved();
  });

  async function checkSaved() {
    try {
      const found = await invoke<SavedPairingInfo | null>("check_saved_pairing");
      if (found) {
        savedPairing = found;
        page = "saved";
      } else {
        page = "wizard";
      }
    } catch (e) {
      // Backend couldn't even check -- fall through to the normal pairing
      // wizard rather than get stuck on "checking...".
      error = String(e);
      page = "wizard";
    }
  }

  async function verifySaved() {
    verifying = true;
    verifyResult = null;
    verifyError = "";
    try {
      await invoke("verify_saved_pairing");
      verifyResult = "ok";
    } catch (e) {
      verifyResult = "failed";
      verifyError = String(e);
    } finally {
      verifying = false;
    }
  }

  // Runs Pair-Verify + bootstraps a Companion control session (plus a live
  // MRP/AirPlay session if one was paired), then switches to the control
  // page. Available from both the saved-pairing screen and right after a
  // fresh pairing finishes.
  async function openControls() {
    error = "";
    try {
      const info = await invoke<ControlInfo>("start_control_session");
      hasLive = info.has_live;
      playback = null;
      controlError = "";
      page = "control";
      // start_control_session replaces the backend's whole ControlState, so
      // any previously loaded filter list -- and creation-mode draft -- is
      // gone too. Re-check for a saved filter now that the fresh state
      // exists, same as checkSaved() does for the pairing itself on mount,
      // and drop the frontend's now-stale view of the draft.
      await checkSavedFilter();
      resetCreation();
    } catch (e) {
      error = String(e);
    }
  }

  // Tries to reload whatever filter file was last picked (persisted
  // backend-side). Leaves the mode off either way -- loading a list must
  // never silently start auto-muting/skipping.
  async function checkSavedFilter() {
    filterSummary = null;
    filterEnabled = false;
    categoryEnabled = {};
    filterError = "";
    try {
      const found = await invoke<FilterSummary | null>("check_saved_filter_file");
      if (found) {
        filterSummary = found;
        categoryEnabled = Object.fromEntries(found.categories.map((c) => [c, true]));
      }
    } catch (e) {
      filterError = String(e);
    }
  }

  async function pickFilterFile() {
    filterBusy = true;
    filterError = "";
    try {
      const path = await open({ multiple: false, filters: [{ name: "Filter list", extensions: ["json"] }] });
      if (!path || Array.isArray(path)) return; // user cancelled
      const summary = await invoke<FilterSummary>("load_filter_file", { path });
      filterSummary = summary;
      filterEnabled = false;
      categoryEnabled = Object.fromEntries(summary.categories.map((c) => [c, true]));
    } catch (e) {
      filterError = String(e);
    } finally {
      filterBusy = false;
    }
  }

  async function toggleFilterEnabled() {
    const next = !filterEnabled;
    filterBusy = true;
    filterError = "";
    try {
      await invoke("set_filter_enabled", { enabled: next });
      filterEnabled = next;
    } catch (e) {
      filterError = String(e);
    } finally {
      filterBusy = false;
    }
  }

  async function toggleCategory(category: string) {
    const next = !categoryEnabled[category];
    filterError = "";
    try {
      await invoke("set_filter_category_enabled", { category, enabled: next });
      categoryEnabled = { ...categoryEnabled, [category]: next };
      // A category flip can also change which cue is "next" -- refresh now
      // rather than waiting up to a second for the poll to catch up.
      await refreshPlayback();
    } catch (e) {
      filterError = String(e);
    }
  }

  // Unlike categoryEnabled, per-cue enabled state isn't tracked separately
  // client-side -- `playback.filter_cues[i].enabled` already comes back
  // fresh from the backend every poll, so that's the single source of
  // truth; toggling just tells the backend and re-polls immediately.
  async function toggleCue(cue: Cue) {
    if (!playback?.filter_match) return;
    filterError = "";
    try {
      await invoke("set_filter_cue_enabled", { title: playback.filter_match, index: cue.index, enabled: !cue.enabled });
      await refreshPlayback();
    } catch (e) {
      filterError = String(e);
    }
  }

  // Backend's `start_control_session` rebuilds ControlState from scratch --
  // including creation-mode's draft -- so the frontend's view of it needs
  // resetting alongside checkSavedFilter() whenever that happens.
  function resetCreation() {
    creationStage = "idle";
    draft = null;
    creationCues = [];
    pendingSkipCategory = null;
    creationError = "";
  }

  async function pickNewDraft() {
    creationError = "";
    try {
      const path = await saveDialog({ filters: [{ name: "Filter list", extensions: ["json"] }], defaultPath: "filter.json" });
      if (!path) return; // user cancelled
      draft = await invoke<DraftSummary>("creation_new_draft", { path });
      creationStage = "recording";
      creationCues = [];
    } catch (e) {
      creationError = String(e);
    }
  }

  async function pickExistingDraft() {
    creationError = "";
    try {
      const path = await open({ multiple: false, filters: [{ name: "Filter list", extensions: ["json"] }] });
      if (!path || Array.isArray(path)) return; // user cancelled
      draft = await invoke<DraftSummary>("creation_open_draft", { path });
      creationStage = "recording";
      await refreshCreationCues();
    } catch (e) {
      creationError = String(e);
    }
  }

  // Re-fetches the draft's cues for whatever's currently playing -- called
  // after every mutation, and reactively (see the $effect below) whenever
  // the title changes while recording, so the table always reflects the
  // title actually on screen.
  async function refreshCreationCues() {
    if (!playback?.title) {
      creationCues = [];
      return;
    }
    try {
      creationCues = await invoke<CreationCue[]>("creation_list_cues", { title: playback.title });
    } catch (e) {
      creationError = String(e);
    }
  }

  $effect(() => {
    if (creationStage === "recording") {
      refreshCreationCues();
    }
  });

  async function markMute(category: string) {
    creationBusy = true;
    creationError = "";
    try {
      await invoke("creation_mark_mute", { category });
      await refreshCreationCues();
    } catch (e) {
      creationError = String(e);
    } finally {
      creationBusy = false;
    }
  }

  // First press on a skip-category button starts its mark; the second
  // press on that *same* button ends it. Other skip buttons are disabled
  // in the markup while pendingSkipCategory is set, so the "already
  // recording a different category" branch below is a safety net, not the
  // normal path.
  async function toggleSkipMark(category: string) {
    creationBusy = true;
    creationError = "";
    try {
      if (pendingSkipCategory === category) {
        await invoke("creation_end_skip_mark");
        pendingSkipCategory = null;
        await refreshCreationCues();
      } else if (pendingSkipCategory === null) {
        await invoke("creation_start_skip_mark", { category });
        pendingSkipCategory = category;
      }
    } catch (e) {
      creationError = String(e);
    } finally {
      creationBusy = false;
    }
  }

  async function cancelSkipMark() {
    creationBusy = true;
    creationError = "";
    try {
      await invoke("creation_cancel_skip_mark");
      pendingSkipCategory = null;
    } catch (e) {
      creationError = String(e);
    } finally {
      creationBusy = false;
    }
  }

  // Pairs with fmtTime -- parses the m:ss the cue table's inputs display
  // back into seconds, or null for anything unrecognized so the caller can
  // reject the edit without touching the backend.
  function parseTime(text: string): number | null {
    const m = /^(\d+):([0-5]?\d)$/.exec(text.trim());
    if (!m) return null;
    return Number(m[1]) * 60 + Number(m[2]);
  }

  async function updateCueTime(cue: CreationCue, field: "start" | "end", text: string) {
    const seconds = parseTime(text);
    if (seconds == null) {
      creationError = `"${text}" isn't a valid m:ss time`;
      return;
    }
    if (!playback?.title) return;
    creationError = "";
    try {
      const start = field === "start" ? seconds : cue.start;
      const end = field === "end" ? seconds : cue.end;
      await invoke("creation_update_cue", { title: playback.title, index: cue.index, start, end });
      await refreshCreationCues();
    } catch (e) {
      creationError = String(e);
    }
  }

  async function deleteCue(cue: CreationCue) {
    if (!playback?.title) return;
    creationError = "";
    try {
      await invoke("creation_delete_cue", { title: playback.title, index: cue.index });
      await refreshCreationCues();
    } catch (e) {
      creationError = String(e);
    }
  }

  function addCustomCategory() {
    const name = newCategoryName.trim();
    if (!name || categories.some((c) => c.name === name)) return;
    categories = [...categories, { name, kind: newCategoryKind }];
    newCategoryName = "";
  }

  // Reloads the draft's own file into the (separate) auto-filter list, so
  // cues just recorded can be tried out live without manually re-picking
  // the file via "Load a different filter file…".
  async function useDraftAsActiveFilter() {
    if (!draft) return;
    filterBusy = true;
    filterError = "";
    try {
      const summary = await invoke<FilterSummary>("load_filter_file", { path: draft.path });
      filterSummary = summary;
      filterEnabled = false;
      categoryEnabled = Object.fromEntries(summary.categories.map((c) => [c, true]));
    } catch (e) {
      filterError = String(e);
    } finally {
      filterBusy = false;
    }
  }

  async function doSkip(seconds: number) {
    controlBusy = true;
    controlError = "";
    try {
      await invoke("control_skip", { seconds });
      // Backend actively re-fetches on every control_playback_status call
      // now, but that's on the next 1s poll tick -- fetch immediately too
      // so the display catches up to the skip right away.
      await refreshPlayback();
    } catch (e) {
      controlError = String(e);
    } finally {
      controlBusy = false;
    }
  }

  async function doMute() {
    controlBusy = true;
    controlError = "";
    try {
      await invoke("control_mute");
    } catch (e) {
      controlError = String(e);
    } finally {
      controlBusy = false;
    }
  }

  async function doUnmute() {
    controlBusy = true;
    controlError = "";
    try {
      await invoke("control_unmute");
    } catch (e) {
      controlError = String(e);
    } finally {
      controlBusy = false;
    }
  }

  // Fetches one playback snapshot -- errors propagate to the caller rather
  // than being handled here, since callers (doSkip, the periodic poll)
  // each have their own busy/error state to manage around it.
  async function refreshPlayback() {
    playback = await invoke<PlaybackStatus | null>("control_playback_status");
  }

  // Polls now-playing status once a second while the control page is open
  // and a live (MRP/AirPlay) transport is available -- there's nothing to
  // poll otherwise, since title/position only ever come from that
  // transport, not Companion. The backend actively re-fetches on every
  // call (see control_playback_status), so this alone keeps the display
  // in sync with skips and remote-triggered pauses/seeks automatically.
  $effect(() => {
    if (page !== "control" || !hasLive) return;
    const id = setInterval(async () => {
      try {
        await refreshPlayback();
      } catch (e) {
        controlError = String(e);
      }
    }, 1000);
    return () => clearInterval(id);
  });

  function fmtTime(seconds: number | null | undefined): string {
    if (seconds == null) return "--:--";
    const total = Math.max(0, Math.round(seconds));
    const m = Math.floor(total / 60);
    const s = total % 60;
    return `${m}:${s.toString().padStart(2, "0")}`;
  }

  async function scan(protocol: Protocol) {
    scanning = true;
    error = "";
    devices = [];
    try {
      devices = await invoke<Device[]>("discover_devices", { protocol });
      if (devices.length === 0) {
        error = `No ${PROTOCOL_LABEL[protocol]} devices found on the network.`;
      }
    } catch (e) {
      error = String(e);
    } finally {
      scanning = false;
    }
  }

  // Re-scan automatically whenever the wizard lands on a new discovery
  // step -- but only once we're actually on the wizard page (not while
  // still checking for, or looking at, a saved pairing).
  // This is called when step or page is changed.
  $effect(() => {
    if (page === "wizard" && isProtocol(step)) {
      scan(step);
    }
  });

  async function pair(protocol: Protocol, device: Device) {
    pairing = true;
    error = "";
    let unlisten: UnlistenFn | undefined;
    try {
      // Must be listening *before* invoking -- the backend can fire
      // pin-requested partway through the still-pending invoke() call.
      unlisten = await listen<Protocol>("pin-requested", (event) => {
        if (event.payload === protocol) {
          awaitingPinFor = protocol;
        }
      });
      await invoke(`pair_${protocol}`, { host: device.host, port: device.port });
      advance();
    } catch (e) {
      error = String(e);
    } finally {
      unlisten?.();
      awaitingPinFor = null;
      pin = "";
      pairing = false;
    }
  }

  async function submitPin() {
    if (!awaitingPinFor) return;
    try {
      await invoke("submit_pin", { protocol: awaitingPinFor, pin });
    } catch (e) {
      error = String(e);
    }
  }

  function advance() {
    const i = STEPS.indexOf(step);
    step = STEPS[i + 1];
  }

  function skip() {
    error = "";
    advance();
  }

  async function save() {
    pairing = true;
    error = "";
    try {
      await invoke("finish_pairing");
      step = "done";
    } catch (e) {
      error = String(e);
    } finally {
      pairing = false;
    }
  }
</script>

<main class="container">
  <h1>Pair an Apple TV</h1>

  {#if page === "checking"}
    <p>Checking for a saved pairing…</p>
  {:else if page === "saved" && savedPairing}
    <section>
      <h2>Saved Apple TV found</h2>
      <p><code>{savedPairing.host}:{savedPairing.port}</code></p>
      <p class="hint">
        MRP: {savedPairing.has_mrp ? "paired" : "not paired"} · AirPlay: {savedPairing.has_airplay ? "paired" : "not paired"}
      </p>

      {#if verifyResult === "ok"}
        <p class="success">✅ Verified — this pairing is still valid.</p>
      {:else if verifyResult === "failed"}
        <p class="error">{verifyError}</p>
      {/if}

      <div class="row">
        <button onclick={verifySaved} disabled={verifying}>{verifying ? "Verifying…" : "Verify"}</button>
        <button onclick={openControls} disabled={verifying}>Open controls</button>
        <button onclick={() => { page = "wizard"; }} disabled={verifying}>Pair a different device</button>
      </div>
    </section>
  {:else if page === "control"}
    <section>
      <h2>Control</h2>
      {#if controlError}
        <p class="error">{controlError}</p>
      {/if}

      {#if hasLive}
        <div class="now-playing">
          <p class="title">{playback?.title ?? "(nothing playing)"}</p>
          <p class="position">
            {fmtTime(playback?.position)} / {fmtTime(playback?.duration)}
            {#if playback}· {playback.playback_state}{/if}
          </p>
          {#if filterSummary && playback}
            <p class="hint">
              {#if playback.filter_action}
                🛡️ {playback.filter_action} — {playback.filter_category}
              {:else if !playback.filter_match}
                no filter list for this title
              {:else if nextCue}
                🛡️ next: {nextCue.action === "mute" ? "🔇 mute" : "⏭️ skip"} at {fmtTime(nextCue.start)}–{fmtTime(nextCue.end)} — {nextCue.category}
                {#if !filterEnabled}(mode off){/if}
              {:else if playback.filter_cues.length > 0}
                no more cues
              {:else}
                🛡️ filter list found for "{playback.filter_match}", no cues
              {/if}
            </p>
          {/if}
        </div>
      {:else}
        <p class="hint">Pair MRP or AirPlay (from the pairing wizard) to unlock mute/unmute and playback info.</p>
      {/if}

      <div class="row">
        <button onclick={() => doSkip(-15)} disabled={controlBusy}>⏪ 15s</button>
        <button onclick={() => doSkip(15)} disabled={controlBusy}>15s ⏩</button>
      </div>

      {#if hasLive}
        <div class="row">
          <button onclick={doMute} disabled={controlBusy}>🔇 Mute</button>
          <button onclick={doUnmute} disabled={controlBusy}>🔊 Unmute</button>
        </div>

        <section class="filter-mode">
          <h3>Auto filter</h3>
          {#if filterError}
            <p class="error">{filterError}</p>
          {/if}

          {#if filterSummary}
            <p class="hint"><code>{filterSummary.path}</code> — {filterSummary.media_count} title{filterSummary.media_count === 1 ? "" : "s"}</p>

            <label class="row">
              <input type="checkbox" checked={filterEnabled} onchange={toggleFilterEnabled} disabled={filterBusy} />
              Enabled
            </label>

            {#if filterSummary.categories.length > 0}
              <p class="hint">
                Categories
                {#if playback?.filter_match}-- expand one to see (and individually toggle) its cues for "{playback.filter_match}"{/if}
              </p>
              <ul class="categories">
                {#each filterSummary.categories as category (category)}
                  {@const cues = cuesByCategory[category] ?? []}
                  {@const isExpanded = expandedCategories[category] ?? true}
                  <li>
                    <div class="category-row">
                      <button
                        type="button"
                        class="disclosure"
                        onclick={() => toggleExpanded(category)}
                        disabled={cues.length === 0}
                        aria-expanded={isExpanded}
                        aria-label={`${isExpanded ? "Collapse" : "Expand"} ${category}`}
                      >
                        {cues.length > 0 ? (isExpanded ? "▾" : "▸") : "·"}
                      </button>
                      <label class="row">
                        <input
                          type="checkbox"
                          checked={categoryEnabled[category] ?? true}
                          onchange={() => toggleCategory(category)}
                        />
                        {category}
                        {#if cues.length > 0}<span class="hint">({cues.length})</span>{/if}
                      </label>
                    </div>

                    {#if isExpanded && cues.length > 0}
                      <ul class="cues">
                        {#each cues as cue (cue.index)}
                          <li
                            class="cue"
                            class:cue-past={playback?.position != null && playback.position >= cue.end}
                            class:cue-active={playback?.position != null &&
                              playback.position >= cue.start &&
                              playback.position < cue.end}
                          >
                            <label class="cue-row">
                              <input type="checkbox" checked={cue.enabled} onchange={() => toggleCue(cue)} />
                              <span class="cue-time">{fmtTime(cue.start)}–{fmtTime(cue.end)}</span>
                              <span class="cue-action">{cue.action === "mute" ? "🔇 mute" : "⏭️ skip"}</span>
                            </label>
                          </li>
                        {/each}
                      </ul>
                    {/if}
                  </li>
                {/each}
              </ul>
            {/if}
          {/if}

          <button onclick={pickFilterFile} disabled={filterBusy}>
            {filterSummary ? "Load a different filter file…" : "Load filter file…"}
          </button>
        </section>

        <section class="creation-mode">
          <h3>Create filter</h3>
          {#if creationError}
            <p class="error">{creationError}</p>
          {/if}

          {#if creationStage === "idle"}
            <p class="hint">Record cue timestamps live from what's currently playing.</p>
            <div class="row">
              <button onclick={pickNewDraft}>Record new filter file…</button>
              <button onclick={pickExistingDraft}>Continue existing draft…</button>
            </div>
          {:else if draft}
            <p class="hint">
              <code>{draft.path}</code> — {draft.media_count} title{draft.media_count === 1 ? "" : "s"}
              {#if !playback?.title}(nothing playing){/if}
            </p>
            <div class="row">
              <button onclick={useDraftAsActiveFilter} disabled={filterBusy}>Use this draft as active filter</button>
              <button onclick={() => { resetCreation(); }} disabled={creationBusy}>Close draft</button>
            </div>
            {#if filterSummary?.path === draft.path}
              <p class="hint">🛡️ this draft is the active auto filter -- cues you record show up there too.</p>
            {:else}
              <p class="hint">Not the active auto filter yet -- use the button above once you're ready to test it.</p>
            {/if}

            <div class="category-buttons">
              {#each categories as c (c.name)}
                <button
                  type="button"
                  class="category-btn"
                  class:recording={pendingSkipCategory === c.name}
                  disabled={creationBusy || !playback?.title || (c.kind === "skip" && pendingSkipCategory !== null && pendingSkipCategory !== c.name)}
                  onclick={() => (c.kind === "mute" ? markMute(c.name) : toggleSkipMark(c.name))}
                >
                  {c.kind === "mute" ? "🔇" : "⏭️"}
                  {c.name}
                  {#if pendingSkipCategory === c.name}(recording — press again to end){/if}
                </button>
              {/each}
            </div>
            {#if pendingSkipCategory}
              <div class="row">
                <button onclick={cancelSkipMark} disabled={creationBusy}>Cancel mark</button>
              </div>
            {/if}

            <div class="add-category row">
              <input placeholder="custom category" bind:value={newCategoryName} />
              <select bind:value={newCategoryKind}>
                <option value="skip">skip</option>
                <option value="mute">mute</option>
              </select>
              <button type="button" onclick={addCustomCategory} disabled={!newCategoryName.trim()}>Add</button>
            </div>

            {#if creationCues.length > 0}
              <p class="hint">Recorded cues for "{playback?.title}" -- edit a time and press Enter/Tab to correct it.</p>
              <ul class="cue-table">
                {#each creationCues as cue (cue.index)}
                  <li class="cue-table-row">
                    <span class="cue-action">{cue.action === "mute" ? "🔇" : "⏭️"} {cue.category}</span>
                    <input
                      class="time-input"
                      value={fmtTime(cue.start)}
                      onchange={(e) => updateCueTime(cue, "start", (e.target as HTMLInputElement).value)}
                    />
                    <span>–</span>
                    <input
                      class="time-input"
                      value={fmtTime(cue.end)}
                      onchange={(e) => updateCueTime(cue, "end", (e.target as HTMLInputElement).value)}
                    />
                    <button type="button" onclick={() => deleteCue(cue)} aria-label="Delete cue">✕</button>
                  </li>
                {/each}
              </ul>
            {:else if playback?.title}
              <p class="hint">No cues recorded yet for "{playback.title}".</p>
            {/if}
          {/if}
        </section>
      {/if}
    </section>
  {:else}
    <ol class="steps">
      {#each ["companion", "mrp", "airplay", "save"] as s (s)}
        <li class:active={step === s} class:done={STEPS.indexOf(step) > STEPS.indexOf(s as Step)}>
          {isProtocol(s as Step) ? PROTOCOL_LABEL[s as Protocol] : "Save"}
        </li>
      {/each}
    </ol>

    {#if error}
      <p class="error">{error}</p>
    {/if}

    {#if isProtocol(step)}
    <section>
      <h2>{PROTOCOL_LABEL[step]} pairing{step !== "companion" ? " (optional)" : ""}</h2>
      {#if step !== "companion"}
        <p class="hint">Needed only for live playback position. Skip if this Apple TV isn't reachable over {PROTOCOL_LABEL[step]}.</p>
      {/if}

      {#if awaitingPinFor === step}
        <form class="row" onsubmit={(e) => { e.preventDefault(); submitPin(); }}>
          <p>Enter the PIN shown on your Apple TV:</p>
          <input inputmode="numeric" autocomplete="one-time-code" bind:value={pin} placeholder="0000" />
          <button type="submit" disabled={!pin}>Submit</button>
        </form>
      {:else}
        <div class="row">
          <button onclick={() => scan(step as Protocol)} disabled={scanning || pairing}>
            {scanning ? "Scanning…" : "Rescan"}
          </button>
          {#if step !== "companion"}
            <button onclick={skip} disabled={pairing}>Skip</button>
          {/if}
        </div>

        {#if devices.length > 0}
          <ul class="devices">
            {#each devices as device (device.host + device.port)}
              <li>
                <button onclick={() => pair(step as Protocol, device)} disabled={pairing}>
                  {device.host}:{device.port}
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      {/if}
    </section>
  {:else if step === "save"}
    <section>
      <h2>Save pairing</h2>
      <p>Ready to save credentials to <code>pairing.store</code>.</p>
      <button onclick={save} disabled={pairing}>{pairing ? "Saving…" : "Save"}</button>
    </section>
  {:else if step === "done"}
    <section>
      <h2>✅ Paired</h2>
      <p>Credentials saved. This Apple TV is ready to control.</p>
      <button onclick={openControls}>Open controls</button>
    </section>
    {/if}
  {/if}

</main>

<style>
:root {
  font-family: Inter, Avenir, Helvetica, Arial, sans-serif;
  font-size: 16px;
  line-height: 24px;
  font-weight: 400;

  color: #0f0f0f;
  background-color: #f6f6f6;

  font-synthesis: none;
  text-rendering: optimizeLegibility;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  -webkit-text-size-adjust: 100%;
}

.container {
  margin: 0 auto;
  max-width: 32rem;
  padding: 3rem 1.5rem;
}

h1 {
  text-align: center;
}

.steps {
  display: flex;
  justify-content: space-between;
  list-style: none;
  padding: 0;
  margin: 2rem 0;
  font-size: 0.85em;
}

.steps li {
  flex: 1;
  text-align: center;
  padding-bottom: 0.5em;
  border-bottom: 3px solid #d8d8d8;
  color: #888;
}

.steps li.done {
  border-color: #396cd8;
  color: #396cd8;
}

.steps li.active {
  border-color: #24c8db;
  color: #0f0f0f;
  font-weight: 600;
}

.error {
  color: #c0392b;
  background: #fdecea;
  border-radius: 8px;
  padding: 0.75em 1em;
}

.success {
  color: #1e7e34;
  background: #e6f6ea;
  border-radius: 8px;
  padding: 0.75em 1em;
}

.hint {
  color: #666;
  font-size: 0.9em;
}

.now-playing {
  text-align: center;
  margin: 1.5em 0;
}

.now-playing .title {
  font-size: 1.2em;
  font-weight: 600;
  margin: 0 0 0.25em;
}

.now-playing .position {
  color: #666;
  font-variant-numeric: tabular-nums;
  margin: 0;
}

.row {
  display: flex;
  gap: 0.5em;
  align-items: center;
  margin: 1em 0;
}

.devices {
  list-style: none;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0.5em;
}

.devices button {
  width: 100%;
  text-align: left;
}

.filter-mode {
  margin-top: 1.5em;
  padding-top: 1em;
  border-top: 1px solid #d8d8d8;
}

.filter-mode h3 {
  margin: 0 0 0.5em;
  font-size: 1em;
}

.categories {
  list-style: none;
  padding: 0;
  margin: 0 0 1em;
  display: flex;
  flex-direction: column;
  gap: 0.35em;
}

.category-row {
  display: flex;
  align-items: center;
  gap: 0.3em;
}

/* Reset the global button styling (background/border/shadow/padding) down
   to a plain inline glyph -- this is a disclosure triangle, not a button. */
.disclosure {
  all: unset;
  width: 1.25em;
  flex-shrink: 0;
  text-align: center;
  cursor: pointer;
  color: #666;
}

.disclosure:disabled {
  cursor: default;
  opacity: 0.35;
}

/* Nested under its category, indented past the disclosure triangle above. */
.categories .cues {
  margin: 0.35em 0 0 1.65em;
}

.cues {
  list-style: none;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0.35em;
  max-height: 12em;
  overflow-y: auto;
}

.cue {
  border-radius: 6px;
  background: #ffffff;
  font-size: 0.9em;
  opacity: 0.55;
}

.cue-row {
  display: flex;
  gap: 0.75em;
  align-items: center;
  padding: 0.4em 0.6em;
  cursor: pointer;
}

.cue-time {
  font-variant-numeric: tabular-nums;
  font-weight: 600;
  min-width: 8em;
}

.cue-active {
  opacity: 1;
  outline: 2px solid #24c8db;
}

.cue-past {
  opacity: 0.3;
}

.creation-mode {
  margin-top: 1.5em;
  padding-top: 1em;
  border-top: 1px solid #d8d8d8;
}

.creation-mode h3 {
  margin: 0 0 0.5em;
  font-size: 1em;
}

.category-buttons {
  display: flex;
  flex-wrap: wrap;
  gap: 0.5em;
  margin: 1em 0;
}

.category-btn.recording {
  outline: 2px solid #24c8db;
  border-color: #24c8db;
}

.add-category {
  margin-bottom: 1em;
}

.add-category input {
  flex: 1;
}

.cue-table {
  list-style: none;
  padding: 0;
  margin: 0.5em 0 0;
  display: flex;
  flex-direction: column;
  gap: 0.35em;
  max-height: 14em;
  overflow-y: auto;
}

.cue-table-row {
  display: flex;
  align-items: center;
  gap: 0.5em;
  padding: 0.4em 0.6em;
  border-radius: 6px;
  background: #ffffff;
  font-size: 0.9em;
}

.cue-table-row .cue-action {
  flex: 1;
}

.time-input {
  width: 4.5em;
  padding: 0.3em 0.5em;
  font-variant-numeric: tabular-nums;
  text-align: center;
}

input,
button {
  border-radius: 8px;
  border: 1px solid transparent;
  padding: 0.6em 1.2em;
  font-size: 1em;
  font-weight: 500;
  font-family: inherit;
  color: #0f0f0f;
  background-color: #ffffff;
  transition: border-color 0.25s;
  box-shadow: 0 2px 2px rgba(0, 0, 0, 0.2);
}

button {
  cursor: pointer;
}

button:hover:not(:disabled) {
  border-color: #396cd8;
}

button:disabled {
  opacity: 0.5;
  cursor: default;
}

input,
button {
  outline: none;
}

@media (prefers-color-scheme: dark) {
  :root {
    color: #f6f6f6;
    background-color: #2f2f2f;
  }

  .steps li {
    border-color: #444;
  }

  .filter-mode {
    border-color: #444;
  }

  .creation-mode {
    border-color: #444;
  }

  .cue {
    background: #0f0f0f98;
  }

  .cue-table-row {
    background: #0f0f0f98;
  }

  .disclosure {
    color: #aaa;
  }

  .error {
    background: #4a1f1c;
    color: #ff8a80;
  }

  .success {
    background: #1c3a24;
    color: #8fe0a4;
  }

  .hint {
    color: #aaa;
  }

  .now-playing .position {
    color: #aaa;
  }

  input,
  button {
    color: #ffffff;
    background-color: #0f0f0f98;
  }
}
</style>
