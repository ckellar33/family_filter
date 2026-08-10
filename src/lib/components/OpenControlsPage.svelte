<script lang="ts">
  // Pure now-playing + transport: title/progress, mute/unmute/skip, and a
  // passive "what's coming up" hint. Picking which filter list is active and
  // toggling its categories/cues now lives entirely in Select Filter --
  // this screen only shows the *result* of that (filter_action/filter_cues
  // already reflect whatever's currently selected there).
  import { session, doSkip, doMute, doUnmute } from "$lib/state/session.svelte";
  import { filterState } from "$lib/state/filter.svelte";
  import { fmtTime } from "$lib/format";

  // Secondary line under Now Playing's main title -- whichever of
  // title/subtitle actually differs from what's shown as the main title
  // (series_name, or title itself if there's no series_name), tried in that
  // order. Deliberately not "subtitle only when there's no series_name":
  // apps disagree about where the episode name goes even when they *do*
  // populate series_name -- e.g. Disney+ appears to leave title equal to
  // the show name and put the actual episode title in subtitle instead, the
  // opposite of Apple's own apps (title = episode, series_name = show) this
  // was originally written against.
  let episodeLine = $derived.by(() => {
    const p = session.playback;
    if (!p) return null;
    const mainTitle = p.series_name ?? p.title;
    for (const candidate of [p.title, p.subtitle]) {
      if (candidate && candidate !== mainTitle) return candidate;
    }
    return null;
  });

  // The single soonest cue that hasn't fully passed yet and would actually
  // fire (category + individual toggle both on) -- what shows up next to
  // the playback position, rather than the whole schedule. filter_cues
  // already arrives sorted by start, so the first match is the soonest one.
  let nextCue = $derived.by(() => {
    const p = session.playback;
    if (!p) return null;
    return p.filter_cues.find((c) => c.enabled && (p.position == null || c.end > p.position)) ?? null;
  });
</script>

<section class="screen">
  {#if session.controlError}
    <p class="banner error">{session.controlError}</p>
  {/if}

  {#if session.hasLive}
    <div class="now-playing">
      <p class="title">{session.playback?.series_name ?? session.playback?.title ?? "Nothing Playing"}</p>
      {#if episodeLine}
        <p class="episode-title">{episodeLine}</p>
      {/if}
      {#if session.playback}
        <p class="subtitle">
          {#if session.playback.app_bundle_id}{session.playback.app_name ?? session.playback.app_bundle_id} · {/if}{session.playback.playback_state}
        </p>
      {/if}
      {#if session.playback?.duration}
        {@const pct = session.playback.position != null ? Math.min(100, (session.playback.position / session.playback.duration) * 100) : 0}
        <div class="progress-track"><div class="progress-fill" style={`width: ${pct}%`}></div></div>
      {/if}
      <p class="position">{fmtTime(session.playback?.position)} / {fmtTime(session.playback?.duration)}</p>
      {#if filterState.filterSummary && session.playback}
        <p class="hint centered">
          {#if session.playback.filter_action}
            🛡️ {session.playback.filter_action} — {session.playback.filter_category}
          {:else if !session.playback.filter_match}
            no filter list for this title
          {:else if nextCue}
            🛡️ next: {nextCue.action === "mute" ? "🔇 mute" : "⏭️ skip"} at {fmtTime(nextCue.start)}–{fmtTime(nextCue.end)} — {nextCue.category}
            {#if !filterState.filterEnabled}(mode off){/if}
          {:else if session.playback.filter_cues.length > 0}
            no more cues
          {:else}
            🛡️ filter list found for "{session.playback.filter_match}", no cues
          {/if}
        </p>
      {/if}
    </div>
  {:else}
    <p class="hint centered">Pair MRP or AirPlay (from Devices) to unlock mute/unmute and playback info.</p>
  {/if}

  <div class="transport-row">
    <button class="icon-btn" onclick={() => doSkip(-15)} disabled={session.controlBusy} aria-label="Back 15 seconds">⏪</button>
    {#if session.hasLive}
      <button class="icon-btn icon-btn-lg" onclick={doMute} disabled={session.controlBusy} aria-label="Mute">🔇</button>
      <button class="icon-btn icon-btn-lg" onclick={doUnmute} disabled={session.controlBusy} aria-label="Unmute">🔊</button>
    {/if}
    <button class="icon-btn" onclick={() => doSkip(15)} disabled={session.controlBusy} aria-label="Forward 15 seconds">⏩</button>
  </div>
</section>
