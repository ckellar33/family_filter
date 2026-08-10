<script lang="ts">
  let { title, poster, onclick }: { title: string; poster: string | null; onclick: () => void } = $props();
</script>

<button type="button" class="poster-tile" {onclick}>
  <span class="poster-art">
    {#if poster}
      <img src={poster} alt="" loading="lazy" />
    {:else}
      <span class="poster-placeholder">🎬</span>
    {/if}
  </span>
  <span class="poster-tile-title">{title}</span>
</button>
