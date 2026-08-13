<script lang="ts">
  // Tab bar glyphs, drawn rather than borrowed from the emoji set: same
  // 1.7px stroke weight, same rounded joins, same 22px box, so the three
  // read as one family at 10.5px label size. `currentColor` throughout, so
  // the active/inactive colour comes from .tab-btn.
  let { name, active = false }: { name: "controls" | "filters" | "create"; active?: boolean } = $props();
</script>

<svg
  class="tab-icon"
  width="22"
  height="22"
  viewBox="0 0 22 22"
  fill="none"
  stroke="currentColor"
  stroke-width="1.7"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {#if name === "controls"}
    <!-- A screen with a play mark: what's on the TV right now. Fills in
         when active so the current tab reads as a solid shape. -->
    <rect x="2.5" y="4" width="17" height="12" rx="2.5" fill={active ? "currentColor" : "none"} />
    <path d="M9.3 8.2 13.2 10 9.3 11.8Z" fill={active ? "var(--bg)" : "currentColor"} stroke="none" />
    <path d="M7.5 19h7" />
  {:else if name === "filters"}
    <!-- Two posters, one behind the other -- the grid, in miniature. -->
    <rect x="7.5" y="3" width="11" height="15" rx="2" fill={active ? "currentColor" : "none"} />
    <path d="M4.5 6v10.5A2.5 2.5 0 0 0 7 19h7.5" />
  {:else}
    <!-- A waveform with a cue mark set into it: recording a moment. -->
    <path d="M3 11h2.6M16.4 11H19" />
    <path d="M8 7.5v7M11 5.5v11M14 8.5v5" stroke-width={active ? "2.4" : "1.7"} />
  {/if}
</svg>
