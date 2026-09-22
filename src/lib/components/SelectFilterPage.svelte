<script lang="ts">
  // Browse-and-configure, VidAngel-style: a poster grid of every title
  // across every filter file the app knows about (see
  // control::list_filter_tiles). Tapping a tile opens straight into a
  // best-guess service variant's master Enabled switch + category/cue tree
  // -- no intermediate picker step -- with a switcher right there in the
  // header to correct the guess if the title has more than one variant
  // (see openTitle's doc comment in filter.svelte.ts).
  import {
    filterState,
    loadTiles,
    loadOnlineTiles,
    openOnlinePreview,
    switchOnlinePreviewService,
    closeOnlinePreview,
    downloadOnlineFilter,
    deleteFilterFile,
    openTitle,
    selectTile,
    addFilterFiles,
    addFilterDirectory,
    toggleFilterEnabled,
    toggleDetailCategory,
    toggleDetailCue,
    updateDetailCueTime,
    deleteDetailCue,
    addDetailCue,
    closeDetail,
    publishDetailOnline,
  } from "$lib/state/filter.svelte";
  import type { Cue, FilterTile } from "$lib/types";
  import { fmtTime, censorWord } from "$lib/format";
  import PosterTile from "$lib/components/PosterTile.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import CueEditorSheet from "$lib/components/CueEditorSheet.svelte";
  import AddCueSheet from "$lib/components/AddCueSheet.svelte";

  let { onRecordInstead }: { onRecordInstead: () => void } = $props();

  // Which cue the editor sheet is open on, if any -- tapping a cue's time
  // (rather than its enabled switch) opens this, same trigger
  // CreateFilterPage's recorded-cues table uses.
  let editingCue = $state<Cue | null>(null);

  async function saveEditedCue(cue: Cue, next: { start: number; end: number }) {
    if (next.start !== cue.start || next.end !== cue.end) await updateDetailCueTime(cue, next.start, next.end);
    editingCue = null;
  }

  async function deleteEditedCue(cue: Cue) {
    await deleteDetailCue(cue);
    editingCue = null;
  }

  // "+ Add cue" sheet, shared by the local detail view and (via
  // addCueFromPreview below) the online preview -- always operates on
  // whatever's open in filterState.detail, since AddCueSheet itself has no
  // notion of which entry it's for.
  let showAddCue = $state(false);

  async function saveNewCue(cue: { start: number; end: number; action: "mute" | "skip"; category: string; word: string | null; note: string | null }) {
    await addDetailCue(cue);
    if (!filterState.detailError) showAddCue = false;
  }

  $effect(() => {
    loadTiles();
  });

  // Which grid is showing when nothing's open in detail: the local library
  // (default) or the shared online one. Only ever fetched once each is
  // first switched to -- switching back to a grid already loaded just shows
  // what's already there rather than re-fetching (matches how `tiles`
  // itself isn't re-loaded on every visit either).
  let gridSource = $state<"local" | "online">("local");

  // The online preview's own "Add to My Filters" button: downloads (see
  // downloadOnlineFilter's doc comment for why that alone doesn't open
  // anything), then switches to "On this device" so the new tile is right
  // there waiting -- the actual confirmation that it landed.
  async function addPreviewToMyFilters() {
    const tile = filterState.previewTile;
    if (!tile) return;
    await downloadOnlineFilter(tile);
    gridSource = "local";
  }

  // The online preview's own "+ Add cue" button: unlike addPreviewToMyFilters
  // (which deliberately lands on the grid, not detail -- see that function's
  // doc comment), this needs the newly-downloaded entry's *detail* view open
  // so AddCueSheet has somewhere (filterState.detail) to actually save
  // against, so it opens straight into it instead.
  async function addCueFromPreview() {
    const tile = filterState.previewTile;
    if (!tile) return;
    await downloadOnlineFilter(tile);
    await openTitle(tile.title);
    showAddCue = true;
  }

  // "On this device"'s own delete action -- a native confirm rather than a
  // custom sheet, since this is the first "delete a whole thing" (as
  // opposed to one cue) action in the app and doesn't need more ceremony
  // than that. Removing a currently-open title closes its detail view too
  // (handled inside deleteFilterFile), so there's nothing left pointing at
  // a file that no longer exists.
  function confirmDeleteTile(tile: FilterTile) {
    if (confirm(`Remove "${tile.title}" from My Filters? This deletes it from this device.`)) {
      deleteFilterFile(tile.path);
    }
  }

  // Search is the primary control on this screen: it filters whichever
  // library is in scope, client-side over the tiles already loaded (both
  // lists are whole-library fetches, so there's nothing to round-trip
  // for). The query deliberately survives a scope switch -- "not in mine,
  // is it online?" is the common move.
  let query = $state("");

  function matches(title: string) {
    const q = query.trim().toLowerCase();
    return q === "" || title.toLowerCase().includes(q);
  }

  let shownTiles = $derived(filterState.tiles.filter((t) => matches(t.title)));
  let shownOnlineTiles = $derived(filterState.onlineTiles.filter((t) => matches(t.title)));

  $effect(() => {
    if (gridSource === "online" && filterState.onlineTiles.length === 0 && !filterState.onlineTilesLoading) {
      loadOnlineTiles();
    }
  });

  // Fetched eagerly (not just on switching to the Online tab) so
  // detailNeedsPublish below has an answer the first time a local title's
  // detail is opened, rather than only once the user happens to have
  // browsed Online first.
  $effect(() => {
    if (filterState.detail && !filterState.onlineTilesEverLoaded && !filterState.onlineTilesLoading) {
      loadOnlineTiles();
    }
  });

  // Whether the open detail's (title, service) entry needs a push to the
  // shared library: either it's been edited this session (hasUnpublishedEdits,
  // set by updateDetailCueTime/deleteDetailCue), or it simply isn't there yet
  // at all -- a title recorded locally (via Create Filter) or added from a
  // file never goes online on its own, so without this a brand-new entry's
  // Publish button would only ever appear after an incidental edit. Reads
  // `false` (rather than "needs publish") until onlineTilesEverLoaded, so it
  // doesn't flash true for an already-published title while the check is
  // still in flight.
  let detailNeedsPublish = $derived.by(() => {
    const detail = filterState.detail;
    if (!detail) return false;
    if (filterState.hasUnpublishedEdits) return true;
    if (!filterState.onlineTilesEverLoaded) return false;
    const tile = filterState.onlineTiles.find((t) => t.title.toLowerCase() === detail.title.toLowerCase());
    const isOnline = tile?.media.some((m) => (m.service ?? "").toLowerCase() === detail.service.toLowerCase()) ?? false;
    return !isOnline;
  });

  // The open detail's cues, grouped by category, for the categories-as-a-
  // tree view -- each category is expandable to show (and individually
  // toggle) just its own cues.
  let cuesByCategory = $derived.by(() => {
    const grouped: Record<string, Cue[]> = {};
    for (const cue of filterState.detail?.cues ?? []) {
      (grouped[cue.category] ??= []).push(cue);
    }
    return grouped;
  });

  // Same grouping, for the read-only online preview -- kept separate from
  // cuesByCategory (rather than made to source from either) since detail and
  // onlinePreview are never open at once and the two views intentionally
  // diverge (no per-cue/category toggles here, nothing to persist).
  let previewCuesByCategory = $derived.by(() => {
    const grouped: Record<string, Cue[]> = {};
    for (const cue of filterState.onlinePreview?.cues ?? []) {
      (grouped[cue.category] ??= []).push(cue);
    }
    return grouped;
  });

  // Which categories are expanded in the tree. Absent means "default" --
  // expanded whenever there's something to show, so the tree opens up
  // ready-to-read rather than making you click through every category
  // after opening a title.
  let expandedCategories = $state<Record<string, boolean>>({});

  function toggleExpanded(category: string) {
    expandedCategories = { ...expandedCategories, [category]: !(expandedCategories[category] ?? true) };
  }

  // Every service variant of whichever tile's open in the online preview --
  // the preview's own "switch service" control, read straight off the
  // already-fetched tile rather than a round trip (see openOnlinePreview).
  let previewServices = $derived((filterState.previewTile?.media ?? []).map((m) => m.service ?? ""));

  // Matches "language" and any "language-*" subcategory (e.g.
  // language-profanity) -- these are the only cues with a `word` worth
  // showing; every other category's pill stays the plain MUTE/SKIP label
  // CueEditorSheet already shows by default.
  function isLanguageCue(cue: Cue) {
    return cue.category === "language" || cue.category.startsWith("language-");
  }
</script>

<section class="screen-stack">
  <!-- Always mounted underneath, even while detail/preview is open on top:
       edgeSwipeBack (actions.ts) looks for this exact class at drag-start
       to parallax it into view from behind the front layer, iOS-style,
       instead of just sliding the front layer away over blank space. Kept
       interactive-only when it's actually on top (inert + aria-hidden
       while a front layer covers it) so it can't steal focus/taps through
       whatever's on top of it. -->
  <div
    class="screen edge-back-layer"
    inert={!!(filterState.detail || filterState.onlinePreview)}
    aria-hidden={!!(filterState.detail || filterState.onlinePreview)}
  >
    <!-- My Filters (this device's local library) vs Online (the shared
         Neon-backed library, see online.rs) -- same tab bar shape as the
         service switcher above, not a NavBar-level thing, since it only
         ever matters within this one screen. -->
    <!-- Search leads; the two libraries are scope tabs under it rather than
         a pair of buttons, since the online library outgrows browsing. -->
    <div class="search-field">
      <span class="search-icon" aria-hidden="true">⌕</span>
      <input type="search" placeholder="Search titles" bind:value={query} autocapitalize="none" autocorrect="off" spellcheck="false" />
      {#if query !== ""}
        <button type="button" class="search-clear" onclick={() => (query = "")} aria-label="Clear search">×</button>
      {/if}
    </div>

    <div class="scope-tabs">
      <button type="button" class="scope-tab" class:selected={gridSource === "local"} onclick={() => (gridSource = "local")}>
        <span class="scope-label">On this device <span class="scope-count">{filterState.tiles.length}</span></span>
        <span class="scope-rule"></span>
      </button>
      <button type="button" class="scope-tab" class:selected={gridSource === "online"} onclick={() => (gridSource = "online")}>
        <span class="scope-label"
          >Online{#if filterState.onlineTiles.length > 0}<span class="scope-count">{filterState.onlineTiles.length}</span>{/if}</span
        >
        <span class="scope-rule"></span>
      </button>
    </div>

    {#if gridSource === "local"}
      {#if filterState.tilesError}
        <p class="banner error">{filterState.tilesError}</p>
      {/if}

      {#if filterState.tilesLoading && filterState.tiles.length === 0}
        <p class="hint centered">Loading filters…</p>
      {:else if filterState.tiles.length === 0}
        <EmptyState
          kind="no-filters"
          onPrimary={addFilterFiles}
          onSecondary={onRecordInstead}
          onTertiary={filterState.folderImportSupported ? addFilterDirectory : undefined}
        />
      {:else}
        <div class="stack">
          <div style="display:flex; gap:9px">
            <button class="btn-secondary" style="min-height:46px" onclick={addFilterFiles}>Add file…</button>
            {#if filterState.folderImportSupported}
              <button class="btn-secondary" style="min-height:46px" onclick={addFilterDirectory}>Add folder…</button>
            {/if}
          </div>
        </div>
        <p class="footnote">
          {#if query.trim() !== ""}
            {shownTiles.length} of {filterState.tiles.length} {filterState.tiles.length === 1 ? "title" : "titles"}
          {:else}
            {filterState.tiles.length} {filterState.tiles.length === 1 ? "title" : "titles"}
          {/if}
        </p>
        {#if shownTiles.length === 0}
          <p class="hint centered">No titles on this device match “{query.trim()}” — try Online.</p>
        {/if}
        <div class="poster-grid">
          {#each shownTiles as tile (tile.title)}
            <!-- Wrapped rather than built into PosterTile itself: the tile
                 stays one big tap target (a <button>), and the delete
                 affordance is a sibling positioned over its corner --
                 nesting a second interactive element inside that button
                 wouldn't be valid HTML. -->
            <div class="poster-tile-wrap">
              <PosterTile title={tile.title} poster={tile.poster} cueCount={tile.cue_count} onclick={() => openTitle(tile.title)} />
              <button
                type="button"
                class="poster-delete"
                onclick={() => confirmDeleteTile(tile)}
                aria-label={`Remove ${tile.title} from My Filters`}
              >
                ✕
              </button>
            </div>
          {/each}
        </div>
      {/if}
    {:else}
      {#if filterState.onlineTilesError}
        <p class="banner error">{filterState.onlineTilesError}</p>
      {/if}

      {#if filterState.downloadingTitle}
        <p class="hint centered">Downloading “{filterState.downloadingTitle}”…</p>
      {/if}

      {#if filterState.onlineTilesLoading && filterState.onlineTiles.length === 0}
        <p class="hint centered">Loading online filters…</p>
      {:else if filterState.onlineTiles.length === 0 && !filterState.onlineTilesError}
        <p class="hint centered">Nothing in the online library yet.</p>
      {:else}
        <p class="footnote">
          {#if query.trim() !== ""}
            {shownOnlineTiles.length} of {filterState.onlineTiles.length} match — tap one to preview it
          {:else}
            {filterState.onlineTiles.length}
            {filterState.onlineTiles.length === 1 ? "title" : "titles"} — tap one to preview it
          {/if}
        </p>
        {#if shownOnlineTiles.length === 0}
          <p class="hint centered">Nothing in the online library matches “{query.trim()}”.</p>
        {/if}
        <div class="poster-grid">
          {#each shownOnlineTiles as tile (tile.title)}
            <PosterTile
              title={tile.title}
              poster={tile.poster}
              cueCount={tile.cue_count}
              onclick={() => openOnlinePreview(tile)}
            />
          {/each}
        </div>
      {/if}
    {/if}
  </div>

  {#if filterState.detail}
    {@const detail = filterState.detail}
    {@const tile = filterState.tiles.find((t) => t.title === detail.title)}
    <div class="screen edge-front-layer">
    {#if filterState.detailError}
      <p class="banner error">{filterState.detailError}</p>
    {/if}

    <!-- Back lives in the page, not the nav bar: the nav keeps the app mark
         and "Title", and this reads as "‹ All titles" above the poster. -->
    <button type="button" class="back-link" onclick={closeDetail}>‹ All titles</button>

    <div class="detail-header">
      <span class="poster-art">
        {#if tile?.poster}
          <img src={tile.poster} alt="" />
        {:else}
          <span class="poster-placeholder">poster art</span>
        {/if}
      </span>
      <div class="detail-header-info">
        <p class="title">{detail.title}</p>
        <p class="hint">{detail.service ? `On ${detail.service}` : "Generic timing (no service specified)"}</p>
      </div>
    </div>

    {#if filterState.serviceOptions.length > 1}
      <p class="section-header">Service — platforms cut this title differently</p>
      <div class="category-buttons">
        {#each filterState.serviceOptions as option (option.service)}
          <button
            type="button"
            class="category-btn"
            class:selected={option.service.toLowerCase() === detail.service.toLowerCase()}
            style="min-height:46px"
            onclick={() => selectTile(option.path, detail.title, option.service)}
          >
            {option.service || "Generic"}
          </button>
        {/each}
      </div>
    {/if}

    <!-- Master switch, promoted out of the list into its own card: it's the
         one control on this screen that decides whether anything happens at
         all, so it shouldn't read like just another row. -->
    <div class="enable-card" class:on={filterState.filterEnabled}>
      <div style="flex:1; min-width:0">
        <p class="title">{filterState.filterEnabled ? "Filter is on" : "Filter is off"}</p>
        <p class="sub">
          {filterState.filterEnabled ? "Applied the moment this title starts" : "Cues stay saved, nothing fires"}
        </p>
      </div>
      <label class="switch">
        <input type="checkbox" checked={filterState.filterEnabled} onchange={toggleFilterEnabled} disabled={filterState.filterBusy} />
        <span class="switch-track"><span class="switch-thumb"></span></span>
      </label>
    </div>

    <!-- Save-button-shaped: only shows up once a cue's actually been
         retimed/deleted since the last publish (or stays up through a
         failed attempt, for retrying) -- separate from the master Enabled
         switch above (that only ever affects this device), since this
         pushes the edit to every other install, live immediately. -->
    {#if detailNeedsPublish || filterState.publishBusy || filterState.publishError || filterState.publishedJustNow}
      <div class="stack">
        {#if detailNeedsPublish || filterState.publishBusy || filterState.publishError}
          <button type="button" class="btn-secondary" style="min-height:46px" onclick={publishDetailOnline} disabled={filterState.publishBusy}>
            {filterState.publishBusy ? "Publishing…" : filterState.hasUnpublishedEdits ? "Publish edits to Online Library" : "Publish to Online Library"}
          </button>
        {/if}
        {#if filterState.publishError}
          <p class="banner error">{filterState.publishError}</p>
        {:else if filterState.publishedJustNow}
          <p class="hint">Published — everyone's Online tab will see this update.</p>
        {/if}
      </div>
    {/if}

    {#if detail.categories.length > 0}
      <p class="section-header">Categories — tap one to see (and individually toggle) its cues</p>
      <ul class="list">
        {#each detail.categories as category (category)}
          {@const cues = cuesByCategory[category] ?? []}
          {@const isExpanded = expandedCategories[category] ?? true}
          <li>
            <div class="list-row category-row">
              <button
                type="button"
                class="category-label"
                onclick={() => toggleExpanded(category)}
                disabled={cues.length === 0}
                aria-expanded={isExpanded}
              >
                <span class="cat-dot" data-cat={category}></span>
                {category}
                {#if cues.length > 0}<span class="hint">{cues.length} {cues.length === 1 ? "cue" : "cues"}</span>{/if}
              </button>
              <button
                type="button"
                class="disclosure"
                class:expanded={isExpanded && cues.length > 0}
                onclick={() => toggleExpanded(category)}
                disabled={cues.length === 0}
                aria-label={`${isExpanded ? "Collapse" : "Expand"} ${category}`}
              >
                {cues.length > 0 ? "›" : "·"}
              </button>
              <label class="switch">
                <input
                  type="checkbox"
                  checked={filterState.categoryEnabled[category] ?? true}
                  onchange={() => toggleDetailCategory(category)}
                />
                <span class="switch-track"><span class="switch-thumb"></span></span>
              </label>
            </div>

            {#if isExpanded && cues.length > 0}
              <ul class="list nested-list">
                {#each cues as cue (cue.index)}
                  <li class="list-row cue-row static" class:cue-past={!cue.enabled}>
                    <button type="button" class="cue-edit-trigger" onclick={() => (editingCue = cue)}>
                      <span class="cue-time">{fmtTime(cue.start)}–{fmtTime(cue.end)}</span>
                      {#if isLanguageCue(cue) && cue.word}
                        <!-- VidAngel-style: the actual recorded word, censored down to its first letter -- see censorWord. -->
                        <span class="cue-pill" data-action={cue.action} aria-label={`${cue.action === "mute" ? "MUTE" : "SKIP"}: ${cue.word}`}
                          >{censorWord(cue.word)}</span
                        >
                      {:else}
                        <span class="cue-pill" data-action={cue.action}>{cue.action === "mute" ? "MUTE" : "SKIP"}</span>
                      {/if}
                    </button>
                    <label class="switch switch-sm">
                      <input type="checkbox" checked={cue.enabled} onchange={() => toggleDetailCue(cue)} />
                      <span class="switch-track"><span class="switch-thumb"></span></span>
                    </label>
                  </li>
                {/each}
              </ul>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    <button type="button" class="btn-secondary" style="min-height:46px" onclick={() => (showAddCue = true)}>+ Add cue</button>
    </div>
  {:else if filterState.onlinePreview}
    {@const preview = filterState.onlinePreview}
    <div class="screen edge-front-layer">
    <!-- Read-only look at an Online tile -- no Enabled switch, no per-
         category/cue toggles (neither concept applies to something that
         isn't loaded as the active filter list), just what's in it and a
         button to actually add it. See openOnlinePreview's doc comment for
         why tapping a tile lands here instead of downloading straight
         away. -->
    {#if filterState.onlinePreviewError}
      <p class="banner error">{filterState.onlinePreviewError}</p>
    {/if}

    <button type="button" class="back-link" onclick={closeOnlinePreview}>‹ Online</button>

    <div class="detail-header">
      <span class="poster-art">
        {#if filterState.previewTile?.poster}
          <img src={filterState.previewTile.poster} alt="" />
        {:else}
          <span class="poster-placeholder">poster art</span>
        {/if}
      </span>
      <div class="detail-header-info">
        <p class="title">{preview.title}</p>
        <p class="hint">{preview.service ? `On ${preview.service}` : "Generic timing (no service specified)"}</p>
      </div>
    </div>

    {#if previewServices.length > 1}
      <p class="section-header">Service — platforms cut this title differently</p>
      <div class="category-buttons">
        {#each previewServices as service (service)}
          <button
            type="button"
            class="category-btn"
            class:selected={service.toLowerCase() === preview.service.toLowerCase()}
            style="min-height:46px"
            onclick={() => switchOnlinePreviewService(service)}
          >
            {service || "Generic"}
          </button>
        {/each}
      </div>
    {/if}

    {#if filterState.previewTile}
      <div class="stack">
        <button
          type="button"
          class="btn-secondary"
          style="min-height:46px"
          onclick={addPreviewToMyFilters}
          disabled={filterState.downloadingTitle !== null}
        >
          {filterState.downloadingTitle ? "Adding…" : "Add to My Filters"}
        </button>
      </div>
    {/if}

    {#if preview.categories.length > 0}
      <p class="section-header">Categories</p>
      <ul class="list">
        {#each preview.categories as category (category)}
          {@const cues = previewCuesByCategory[category] ?? []}
          {@const isExpanded = expandedCategories[category] ?? true}
          <li>
            <div class="list-row category-row">
              <button
                type="button"
                class="category-label"
                onclick={() => toggleExpanded(category)}
                disabled={cues.length === 0}
                aria-expanded={isExpanded}
              >
                <span class="cat-dot" data-cat={category}></span>
                {category}
                {#if cues.length > 0}<span class="hint">{cues.length} {cues.length === 1 ? "cue" : "cues"}</span>{/if}
              </button>
              <button
                type="button"
                class="disclosure"
                class:expanded={isExpanded && cues.length > 0}
                onclick={() => toggleExpanded(category)}
                disabled={cues.length === 0}
                aria-label={`${isExpanded ? "Collapse" : "Expand"} ${category}`}
              >
                {cues.length > 0 ? "›" : "·"}
              </button>
            </div>

            {#if isExpanded && cues.length > 0}
              <ul class="list nested-list">
                {#each cues as cue (cue.index)}
                  <li class="list-row cue-row static">
                    <span class="cue-time">{fmtTime(cue.start)}–{fmtTime(cue.end)}</span>
                    {#if isLanguageCue(cue) && cue.word}
                      <span class="cue-pill" data-action={cue.action} aria-label={`${cue.action === "mute" ? "MUTE" : "SKIP"}: ${cue.word}`}
                        >{censorWord(cue.word)}</span
                      >
                    {:else}
                      <span class="cue-pill" data-action={cue.action}>{cue.action === "mute" ? "MUTE" : "SKIP"}</span>
                    {/if}
                  </li>
                {/each}
              </ul>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    <button type="button" class="btn-secondary" style="min-height:46px" onclick={addCueFromPreview}>+ Add cue</button>
    </div>
  {/if}
</section>

{#if editingCue}
  {@const cue = editingCue}
  <CueEditorSheet
    start={cue.start}
    end={cue.end}
    category={cue.category}
    action={cue.action}
    onSave={(next) => saveEditedCue(cue, next)}
    onDelete={() => deleteEditedCue(cue)}
    onClose={() => (editingCue = null)}
  />
{/if}

{#if showAddCue}
  <AddCueSheet busy={filterState.addCueBusy} onSave={saveNewCue} onClose={() => (showAddCue = false)} />
{/if}
