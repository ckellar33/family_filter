<script lang="ts">
  let {
    title,
    poster,
    cueCount = null,
    confirmed = false,
    onclick,
  }: {
    title: string;
    poster: string | null;
    cueCount?: number | null;
    confirmed?: boolean;
    onclick: () => void;
  } = $props();
</script>

<button type="button" class="poster-tile" {onclick}>
  <span class="poster-art">
    {#if poster}
      <img src={poster} alt="" loading="lazy" />
    {:else}
      <!-- Striped placeholder rather than an icon: it reads as "artwork
           we haven't got" instead of as a broken tile. -->
      <span class="poster-placeholder">poster art</span>
    {/if}
    {#if cueCount != null}
      <span class="poster-badge">{cueCount} {cueCount === 1 ? "CUE" : "CUES"}</span>
    {/if}
  </span>
  <span class="poster-tile-title">
    {#if confirmed}<span class="poster-confirmed" title="Confirmed" aria-label="Confirmed">★</span>{/if}{title}
  </span>
</button>
