<script lang="ts">
  // Shell/orchestrator: decides which top-level screen is showing (Devices,
  // or one of the three tabs) and wires the sticky NavBar/TabBar to it. Each
  // screen's own logic lives in src/lib/components + src/lib/state -- this
  // file is just the composition root.
  //
  // Starts on the Select Filter tab, but the launch-time auto-connect effect
  // below jumps to Now Playing the moment it actually lands a connection --
  // so a cold launch with a reachable last-used device goes straight to
  // Controls, same as tapping a device card in Devices does, while a launch
  // with nothing to reconnect to (or an unreachable device) just leaves you
  // on Select Filter with nothing forcing you into Devices either; get
  // there via the tab bar or the NavBar's Devices button, same as any other
  // screen. `launching` (below) holds the whole tab area on a splash until
  // that check resolves either way -- rendering Select Filter immediately
  // and only *then* swapping to Controls once the connect finishes made the
  // launch visibly flash Select Filter first every time a reconnect
  // succeeded.
  import { session, checkSaved, openControls, refreshPlayback, STEPS } from "$lib/state/session.svelte";
  import { checkSavedFilter, filterState, closeDetail, selectTile, checkAvailableForPlayback } from "$lib/state/filter.svelte";
  import { resetCreation } from "$lib/state/creation.svelte";
  import type { Tab } from "$lib/types";
  import NavBar from "$lib/components/NavBar.svelte";
  import TabBar from "$lib/components/TabBar.svelte";
  import DevicesPage from "$lib/components/DevicesPage.svelte";
  import OpenControlsPage from "$lib/components/OpenControlsPage.svelte";
  import SelectFilterPage from "$lib/components/SelectFilterPage.svelte";
  import CreateFilterPage from "$lib/components/CreateFilterPage.svelte";

  let activeTab = $state<Tab>("select-filter");
  // Manually opened via NavBar's Devices button (or Open Controls' "connect
  // a device" prompt) -- the *only* thing that shows Devices now; nothing
  // auto-opens it on mount anymore.
  let devicesOpen = $state(false);
  // True only for the launch-time auto-connect attempt below -- see its
  // comment for why the tab area stays on a splash instead of Select Filter
  // while this is true.
  let launching = $state(true);

  // Runs Pair-Verify + bootstraps the control session for saved device
  // `id`, then (on success) brings in the other two modules' post-session
  // setup, and -- unlike the launch-time auto-connect effect below -- jumps
  // to the Controls tab, since every caller here is an explicit "connect
  // now" decision made from within Devices: a manual device tap (initial
  // connect *or* switching devices while one is already active),
  // DevicesPage's own auto-connect-to-last-used retry on mount, or
  // finishing the pairing wizard.
  async function openControlsFlow(id: string) {
    const ok = await openControls(id);
    if (!ok) return;
    await checkSavedFilter();
    resetCreation();
    devicesOpen = false;
    activeTab = "controls";
  }

  function openDevices() {
    devicesOpen = true;
  }

  // Auto-connect to the last-used device on launch, VidAngel-style, then
  // land on Now Playing the same way a manual device tap would (see
  // openControlsFlow above) -- never opens Devices itself, though.
  // DevicesPage still does its own version of this same check when it
  // mounts (a manual Devices tap), which acts as a retry if this one below
  // failed (device asleep/off Wi-Fi at launch) or there was nothing to
  // reconnect to yet. Runs exactly once: this component mounts for the
  // app's whole lifetime, and nothing read synchronously before the first
  // `await` is reactive, so there's nothing here for `$effect` to
  // re-subscribe to and re-fire on.
  $effect(() => {
    (async () => {
      try {
        await checkSaved();
        if (session.page === "saved" && session.lastDeviceId && session.savedDevices.some((d) => d.id === session.lastDeviceId)) {
          const ok = await openControls(session.lastDeviceId);
          if (ok) {
            await checkSavedFilter();
            resetCreation();
            activeTab = "controls";
          }
        }
      } finally {
        launching = false;
      }
    })();
  });

  // Polls now-playing status once a second while a control session is open
  // and a live (MRP/AirPlay) transport is available -- kept here (rather
  // than per-tab) so title/position keep advancing in the background even
  // while looking at Select Filter or Create Filter.
  $effect(() => {
    if (session.page !== "control" || !session.hasLive) return;
    const id = setInterval(async () => {
      try {
        await refreshPlayback();
      } catch (e) {
        session.controlError = String(e);
      }
    }, 1000);
    return () => clearInterval(id);
  });

  // Ticks session.playback's *display* forward smoothly between the polls
  // above -- see livePosition() in session.svelte.ts, the sole consumer.
  // Same gating as the poll effect (no point ticking a position nothing is
  // showing), and deliberately much faster than the 1s poll interval itself
  // since this only drives a local interpolation, not a real device
  // round trip.
  $effect(() => {
    if (session.page !== "control" || !session.hasLive) return;
    const id = setInterval(() => {
      session.tick++;
    }, 200);
    return () => clearInterval(id);
  });

  // Re-checks whether a filter is available for whatever's playing now
  // (Open Controls' "tap to enable" banner) whenever the title, the app
  // it's playing in, or the enabled state changes -- kept here rather than
  // in OpenControlsPage so it keeps working even while looking at a
  // different tab, same reasoning as the playback poll above.
  $effect(() => {
    session.playback?.title;
    session.playback?.app_name;
    filterState.filterEnabled;
    if (session.page === "control") {
      checkAvailableForPlayback();
    }
  });

  // What Open Controls' banner does when tapped: loads that entry as
  // active and jumps to Select Filter so the user can review it and flip
  // Enabled themselves -- same "auto-load, never auto-arm" rule every other
  // filter-loading path in this app follows.
  async function enableAvailableFilter() {
    const hint = filterState.availableHint;
    if (!hint) return;
    await selectTile(hint.path, hint.title, hint.service);
    activeTab = "select-filter";
  }

  // Purely cosmetic label for the sticky nav bar -- doesn't drive any
  // behavior, just names whichever screen is currently showing.
  let navTitle = $derived.by(() => {
    if (launching) return "Family Filter";
    if (devicesOpen) {
      if (session.page === "checking") return "Family Filter";
      if (session.page === "wizard") {
        if (session.step === "save") return "Save pairing";
        if (session.step === "done") return "Paired";
        return "Pair an Apple TV";
      }
      return "Devices";
    }
    if (activeTab === "controls") return "Now Playing";
    if (activeTab === "select-filter") return filterState.detail ? "Title" : "Filters";
    return "Create Filter";
  });

  // The Devices button doubles as the connection indicator -- the name of
  // whatever's connected, or a grey "Not connected" when nothing is.
  let connected = $derived(session.page === "control");
  let deviceLabel = $derived(connected ? (session.activeDevice?.name ?? "Connected") : "Not connected");

  // Whether the sticky nav bar's back button should show for the current
  // screen. There's no real history stack -- each screen has exactly one
  // well-defined "back", mirrored by goBack() below. Devices always has
  // *something* to back out of (an earlier wizard step, or the overlay
  // itself), so this is unconditionally true whenever it's open. The title
  // detail is the exception: its back lives in the page ("‹ All titles"),
  // so the nav bar keeps the app mark instead of a back button.
  let canGoBack = $derived(devicesOpen);

  function goBack() {
    session.error = "";
    if (devicesOpen) {
      if (session.page === "wizard") {
        if (session.step === "companion") {
          if (session.savedDevices.length > 0) {
            session.page = "saved";
          } else {
            // Nothing behind this step to go back to -- close the overlay
            // instead of leaving Back a dead end.
            devicesOpen = false;
          }
          return;
        }
        const i = STEPS.indexOf(session.step);
        session.step = STEPS[i - 1];
        return;
      }
      devicesOpen = false;
      return;
    }
    if (activeTab === "select-filter" && filterState.detail) {
      closeDetail();
    }
  }
</script>

<div class="phone-shell">
  <main class="canvas">
    <NavBar
      title={navTitle}
      {canGoBack}
      onBack={goBack}
      showDevices={!devicesOpen && !launching}
      onDevices={openDevices}
      {connected}
      {deviceLabel}
    />

    <div class="content" class:with-tabbar={!devicesOpen && !launching}>
      {#if launching}
        <section class="screen">
          <p class="hint centered">Checking for a saved device…</p>
        </section>
      {:else if devicesOpen}
        <DevicesPage sessionActive={session.page === "control"} onOpenControls={openControlsFlow} onClose={() => { devicesOpen = false; }} />
      {:else if activeTab === "controls"}
        <OpenControlsPage onEnableFilter={enableAvailableFilter} onOpenDevices={openDevices} />
      {:else if activeTab === "select-filter"}
        <SelectFilterPage onRecordInstead={() => { activeTab = "create-filter"; }} />
      {:else}
        <CreateFilterPage />
      {/if}
    </div>

    {#if !devicesOpen && !launching}
      <TabBar active={activeTab} onSelect={(tab) => { activeTab = tab; }} />
    {/if}
  </main>
</div>
