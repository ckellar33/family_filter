<script lang="ts">
  // Browse-and-configure, VidAngel-style: a poster grid of every title
  // across every filter file the app knows about (see
  // control::list_filter_tiles), and once a tile is tapped, that title's
  // streaming-service badges + master Enabled switch + category/cue tree --
  // picking *and* configuring a filter happen on this one screen.
  import {
    filterState,
    loadTiles,
    selectTile,
    addFilterFiles,
    addFilterDirectory,
    toggleFilterEnabled,
    toggleDetailCategory,
    toggleDetailCue,
  } from "$lib/state/filter.svelte";
  import type { Cue } from "$lib/types";
  import { fmtTime } from "$lib/format";
  import PosterTile from "$lib/components/PosterTile.svelte";

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

    <div class="detail-header">
      <span class="poster-art">
        {#if tile?.poster}
          <img src={tile.poster} alt="" />
        {:else}
          <span class="poster-placeholder">🎬</span>
        {/if}
      </span>
      <div class="detail-header-info">
        <p class="title">{detail.title}</p>
        {#if detail.services.length > 0}
          <div class="provider-badges">
            {#each detail.services as service (service)}
              <span class="provider-badge">{service}</span>
            {/each}
          </div>
        {:else}
          <p class="hint">No service tagged for this title yet -- add one from Create Filter.</p>
        {/if}
      </div>
    </div>

    <ul class="list">
      <li class="list-row">
        <span>Enabled</span>
        <label class="switch">
          <input type="checkbox" checked={filterState.filterEnabled} onchange={toggleFilterEnabled} disabled={filterState.filterBusy} />
          <span class="switch-track"><span class="switch-thumb"></span></span>
        </label>
      </li>
    </ul>

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
                class="disclosure"
                class:expanded={isExpanded && cues.length > 0}
                onclick={() => toggleExpanded(category)}
                disabled={cues.length === 0}
                aria-expanded={isExpanded}
                aria-label={`${isExpanded ? "Collapse" : "Expand"} ${category}`}
              >
                {cues.length > 0 ? "›" : "·"}
              </button>
              <button type="button" class="category-label" onclick={() => toggleExpanded(category)} disabled={cues.length === 0}>
                {category}
                {#if cues.length > 0}<span class="hint">({cues.length})</span>{/if}
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
                  <li class="list-row cue-row">
                    <span class="cue-time">{fmtTime(cue.start)}–{fmtTime(cue.end)}</span>
                    <span class="cue-action">{cue.action === "mute" ? "🔇 mute" : "⏭️ skip"}</span>
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

    <div class="stack">
      <button class="btn-secondary" onclick={addFilterFiles}>Add Filter File(s)…</button>
      <button class="btn-secondary" onclick={addFilterDirectory}>Add Filter Folder…</button>
    </div>

    {#if filterState.tilesLoading && filterState.tiles.length === 0}
      <p class="hint centered">Loading filters…</p>
    {:else if filterState.tiles.length === 0}
      <p class="hint centered">No filter files yet -- add one above, or record one from Create Filter.</p>
    {:else}
      <div class="poster-grid">
        {#each filterState.tiles as tile (tile.title)}
          <PosterTile title={tile.title} poster={tile.poster} onclick={() => selectTile(tile.path, tile.title)} />
        {/each}
      </div>
    {/if}
  {/if}
</section>
