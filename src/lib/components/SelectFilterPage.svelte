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
  } from "$lib/state/filter.svelte";
  import type { Cue } from "$lib/types";
  import { fmtTime } from "$lib/format";
  import PosterTile from "$lib/components/PosterTile.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import CueEditorSheet from "$lib/components/CueEditorSheet.svelte";

  let { onRecordInstead }: { onRecordInstead: () => void } = $props();

  // Which cue the editor sheet is open on, if any -- tapping a cue's time/
  // pill (rather than its enabled switch) opens this, same trigger
  // CreateFilterPage's recorded-cues table uses.
  let editingCue = $state<Cue | null>(null);

  async function saveEditedCue(cue: Cue, next: { start: number; end: number }) {
    if (next.start !== cue.start) await updateDetailCueTime(cue, "start", fmtTime(next.start));
    if (next.end !== cue.end) await updateDetailCueTime(cue, "end", fmtTime(next.end));
    editingCue = null;
  }

  async function deleteEditedCue(cue: Cue) {
    await deleteDetailCue(cue);
    editingCue = null;
  }

  $effect(() => {
    loadTiles();
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
                      <span class="cue-pill" data-action={cue.action}>{cue.action === "mute" ? "MUTE" : "SKIP"}</span>
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
    {#if filterState.tilesError}
      <p class="banner error">{filterState.tilesError}</p>
    {/if}

    {#if filterState.tilesLoading && filterState.tiles.length === 0}
      <p class="hint centered">Loading filters…</p>
    {:else if filterState.tiles.length === 0}
      <EmptyState kind="no-filters" onPrimary={addFilterFiles} onSecondary={onRecordInstead} />
    {:else}
      <div class="stack">
        <div style="display:flex; gap:9px">
          <button class="btn-secondary" style="min-height:46px" onclick={addFilterFiles}>Add file…</button>
          <button class="btn-secondary" style="min-height:46px" onclick={addFilterDirectory}>Add folder…</button>
        </div>
      </div>
      <p class="footnote">{filterState.tiles.length} {filterState.tiles.length === 1 ? "title" : "titles"}</p>
      <div class="poster-grid">
        {#each filterState.tiles as tile (tile.title)}
          <PosterTile title={tile.title} poster={tile.poster} cueCount={tile.cue_count} onclick={() => openTitle(tile.title)} />
        {/each}
      </div>
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
