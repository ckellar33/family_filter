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
    downloadOnlineFilter,
    openTitle,
    selectTile,
    addFilterFiles,
    addFilterDirectory,
    toggleFilterEnabled,
    toggleDetailCategory,
    toggleDetailCue,
    updateDetailCueTime,
    deleteDetailCue,
    closeDetail,
    publishDetailOnline,
  } from "$lib/state/filter.svelte";
  import type { Cue } from "$lib/types";
  import { fmtTime, censorWord } from "$lib/format";
  import PosterTile from "$lib/components/PosterTile.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import CueEditorSheet from "$lib/components/CueEditorSheet.svelte";

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

  $effect(() => {
    loadTiles();
  });

  // Which grid is showing when nothing's open in detail: the local library
  // (default) or the shared online one. Only ever fetched once each is
  // first switched to -- switching back to a grid already loaded just shows
  // what's already there rather than re-fetching (matches how `tiles`
  // itself isn't re-loaded on every visit either).
  let gridSource = $state<"local" | "online">("local");

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

  // Which categories are expanded in the tree. Absent means "default" --
  // expanded whenever there's something to show, so the tree opens up
  // ready-to-read rather than making you click through every category
  // after opening a title.
  let expandedCategories = $state<Record<string, boolean>>({});

  function toggleExpanded(category: string) {
    expandedCategories = { ...expandedCategories, [category]: !(expandedCategories[category] ?? true) };
  }

  // Matches "language" and any "language-*" subcategory (e.g.
  // language-profanity) -- these are the only cues with a `word` worth
  // showing; every other category's pill stays the plain MUTE/SKIP label
  // CueEditorSheet already shows by default.
  function isLanguageCue(cue: Cue) {
    return cue.category === "language" || cue.category.startsWith("language-");
  }
</script>

<section class="screen">
  {#if filterState.detail}
    {@const detail = filterState.detail}
    {@const tile = filterState.tiles.find((t) => t.title === detail.title)}
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
    {#if filterState.hasUnpublishedEdits || filterState.publishBusy || filterState.publishError || filterState.publishedJustNow}
      <div class="stack">
        {#if filterState.hasUnpublishedEdits || filterState.publishBusy || filterState.publishError}
          <button type="button" class="btn-secondary" style="min-height:46px" onclick={publishDetailOnline} disabled={filterState.publishBusy}>
            {filterState.publishBusy ? "Publishing…" : "Publish edits to Online Library"}
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
  {:else}
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
            <PosterTile title={tile.title} poster={tile.poster} cueCount={tile.cue_count} onclick={() => openTitle(tile.title)} />
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
            {shownOnlineTiles.length} of {filterState.onlineTiles.length} match — tap one to add it to this device
          {:else}
            {filterState.onlineTiles.length}
            {filterState.onlineTiles.length === 1 ? "title" : "titles"} — tap one to add it to this device
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
              onclick={() => downloadOnlineFilter(tile)}
            />
          {/each}
        </div>
      {/if}
    {/if}
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
