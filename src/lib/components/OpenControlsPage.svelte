<script lang="ts">
  // Pure now-playing + transport: title/progress, mute/unmute/skip, and a
  // passive "what's coming up" hint. Picking which filter list is active and
  // toggling its categories/cues now lives entirely in Select Filter --
  // this screen only shows the *result* of that (filter_action/filter_cues
  // already reflect whatever's currently selected there).
  import { session, doSkip, doMute, doUnmute, doButton } from "$lib/state/session.svelte";
  import { filterState } from "$lib/state/filter.svelte";
  import { fmtTime } from "$lib/format";
  import EmptyState from "$lib/components/EmptyState.svelte";

  let { onEnableFilter, onOpenDevices }: { onEnableFilter: () => void; onOpenDevices: () => void } = $props();

  // The backend has no mute-status query, so this tracks the last command
  // sent rather than device truth -- enough to light the Mute button and
  // let one button do both directions instead of two competing ones.
  let muted = $state(false);

  async function toggleMute() {
    if (muted) {
      await doUnmute();
      muted = false;
    } else {
      await doMute();
      muted = true;
    }
  }

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

  // The cue firing *right now*, if any -- filter_action is only ever set
  // while auto-filter mode is on, so this is the honest "something is
  // happening to your audio/video this second" signal.
  let firing = $derived(session.playback?.filter_action ?? null);

  // Status chip on the now-playing card: what the filter is doing, in one
  // word, without needing to read the sentence underneath.
  let shieldState = $derived(
    firing === "mute" ? "mute" : firing === "skip" ? "skip" : filterState.filterEnabled ? "on" : "off",
  );
  let shieldLabel = $derived(
    shieldState === "mute"
      ? "MUTING"
      : shieldState === "skip"
        ? "SKIPPING"
        : shieldState === "on"
          ? "FILTER ON"
          : "FILTER OFF",
  );

  // Where the next-cue banner's two lines come from -- the one place that
  // has to reconcile "firing now" / "coming up" / "nothing loaded" /
  // "matched but no cues", so the markup below stays flat.
  let banner = $derived.by(() => {
    const p = session.playback;
    if (!p || !filterState.filterSummary) return null;
    if (firing) {
      return {
        action: firing,
        badge: "NOW",
        title: `${firing === "mute" ? "Muting" : "Skipping"} — ${p.filter_category}`,
        sub: "Back to normal when this cue ends",
      };
    }
    if (!p.filter_match) {
      return { action: null, badge: "—", title: "No filter list for this title", sub: "Pick one from Filters, or record one" };
    }
    if (nextCue) {
      return {
        action: nextCue.action,
        badge: nextCue.action === "mute" ? "MUTE" : "SKIP",
        title: `Next: ${nextCue.action} at ${fmtTime(nextCue.start)}`,
        sub: `${fmtTime(nextCue.start)}–${fmtTime(nextCue.end)} · ${nextCue.category}${filterState.filterEnabled ? "" : " · mode off"}`,
      };
    }
    if (p.filter_cues.length > 0) {
      return { action: null, badge: "—", title: "No more cues", sub: "Enjoy the rest of it" };
    }
    return { action: null, badge: "—", title: `Filter list found for "${p.filter_match}"`, sub: "No cues recorded in it yet" };
  });
</script>

{#if session.page !== "control"}
  <!-- No active control session -- e.g. fresh launch (Devices now owns
       connecting, not this screen), a saved pairing that failed to
       reconnect, or the user backed out of pairing. Nothing below this
       (remote buttons, mute/skip) works without one, so don't render it;
       point at Devices instead of showing controls that'll just error. -->
  <EmptyState
    kind={session.controlError ? "lost" : "no-device"}
    detail={session.controlError}
    onPrimary={onOpenDevices}
    onSecondary={onOpenDevices}
  />
{:else}
  <section class="screen">
    {#if session.controlError}
      <p class="banner error">{session.controlError}</p>
    {/if}

    {#if filterState.availableHint}
      <button type="button" class="banner-link" onclick={onEnableFilter}>
        A filter is available for "{filterState.availableHint.title}" on {filterState.availableHint.service} — tap to enable
      </button>
    {/if}

    {#if session.hasLive}
      {@const p = session.playback}
      <div class="now-playing">
        <p class="subtitle">
          {#if p?.app_bundle_id}{p.app_name ?? p.app_bundle_id} · {/if}{p?.playback_state ?? "idle"}
          <span class="shield" data-state={shieldState}>{shieldLabel}</span>
        </p>
        <p class="title">{p?.series_name ?? p?.title ?? "Nothing Playing"}</p>
        {#if episodeLine}
          <p class="episode-title">{episodeLine}</p>
        {/if}

        {#if p?.duration}
          {@const pct = p.position != null ? Math.min(100, (p.position / p.duration) * 100) : 0}
          <div class="progress-track">
            <div class="progress-fill" style={`width: ${pct}%`}></div>
            <!-- One pip per cue, so the whole shape of the movie's filtering
                 is readable at a glance instead of one cue at a time. -->
            {#each p.filter_cues as cue (cue.index)}
              <span
                class="cue-mark"
                data-action={cue.action}
                data-off={!cue.enabled}
                style={`left: ${Math.min(100, (cue.start / p.duration) * 100)}%`}
              ></span>
            {/each}
          </div>
        {/if}
        <p class="position"><span>{fmtTime(p?.position)}</span><span>{fmtTime(p?.duration)}</span></p>
      </div>

      {#if banner}
        <div class="next-cue" class:firing={!!firing}>
          <span class="next-cue-badge" data-action={banner.action}>{banner.badge}</span>
          <div style="flex:1; min-width:0">
            <p class="title">{banner.title}</p>
            <p class="sub">{banner.sub}</p>
          </div>
        </div>
      {/if}
    {:else}
      <EmptyState kind="companion-only" onPrimary={onOpenDevices} onSecondary={() => {}} />
    {/if}

    <div class="transport-row">
      <button class="icon-btn" onclick={() => doSkip(-15)} disabled={session.controlBusy} aria-label="Back 15 seconds">
        <span>↺</span><span>15s</span>
      </button>
      <button class="icon-btn icon-btn-lg" onclick={() => doButton("play_pause")} disabled={session.controlBusy} aria-label="Play or pause">
        <span>⏯</span>
      </button>
      {#if session.hasLive}
        <button class="icon-btn" aria-pressed={muted} onclick={toggleMute} disabled={session.controlBusy} aria-label={muted ? "Unmute" : "Mute"}>
          <span>{muted ? "🔇" : "🔊"}</span><span>{muted ? "Muted" : "Mute"}</span>
        </button>
      {/if}
      <button class="icon-btn" onclick={() => doSkip(15)} disabled={session.controlBusy} aria-label="Forward 15 seconds">
        <span>↻</span><span>15s</span>
      </button>
    </div>

    <p class="section-header">Remote</p>
    <!-- Siri Remote button ring -- discrete presses only (Companion's `_hidC`
         HID commands), deliberately no touchpad swipe/drag gesture. -->
    <div class="dpad">
      <button class="icon-btn dpad-up" onclick={() => doButton("up")} disabled={session.controlBusy} aria-label="Up">▲</button>
      <button class="icon-btn dpad-left" onclick={() => doButton("left")} disabled={session.controlBusy} aria-label="Left">◀</button>
      <button class="icon-btn dpad-select" onclick={() => doButton("select")} disabled={session.controlBusy} aria-label="Select">OK</button>
      <button class="icon-btn dpad-right" onclick={() => doButton("right")} disabled={session.controlBusy} aria-label="Right">▶</button>
      <button class="icon-btn dpad-down" onclick={() => doButton("down")} disabled={session.controlBusy} aria-label="Down">▼</button>
    </div>

    <div class="transport-row">
      <button class="icon-btn" onclick={() => doButton("menu")} disabled={session.controlBusy}>Menu</button>
      <button class="icon-btn" onclick={() => doButton("home")} disabled={session.controlBusy}>Home</button>
    </div>
  </section>
{/if}
