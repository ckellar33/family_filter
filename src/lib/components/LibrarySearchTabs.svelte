<script lang="ts">
  // Search + scope-tabs header for SelectFilterPage's browse screen, split
  // out so the "what am I browsing" controls stay separate from whatever
  // grid/content is actually shown underneath them. Has no opinion on what
  // "local"/"online" render as -- that's still the parent's job.
  let {
    query = $bindable(""),
    gridSource = $bindable("local"),
    localCount,
    onlineCount,
  }: {
    query?: string;
    gridSource?: "local" | "online";
    localCount: number;
    onlineCount: number;
  } = $props();
</script>

<div class="search-field">
  <span class="search-icon" aria-hidden="true">⌕</span>
  <input type="search" placeholder="Search titles" bind:value={query} autocapitalize="none" autocorrect="off" spellcheck="false" />
  {#if query !== ""}
    <button type="button" class="search-clear" onclick={() => (query = "")} aria-label="Clear search">×</button>
  {/if}
</div>

<div class="scope-tabs">
  <button type="button" class="scope-tab" class:selected={gridSource === "local"} onclick={() => (gridSource = "local")}>
    <span class="scope-label">On this device <span class="scope-count">{localCount}</span></span>
    <span class="scope-rule"></span>
  </button>
  <button type="button" class="scope-tab" class:selected={gridSource === "online"} onclick={() => (gridSource = "online")}>
    <span class="scope-label">Online{#if onlineCount > 0}<span class="scope-count">{onlineCount}</span>{/if}</span>
    <span class="scope-rule"></span>
  </button>
</div>
