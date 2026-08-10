<script lang="ts">
  // Shell/orchestrator: decides which top-level screen is showing (the
  // Devices flow, or one of the three tabs once a control session is
  // active) and wires the sticky NavBar/TabBar to it. Each screen's own
  // logic lives in src/lib/components + src/lib/state -- this file is just
  // the composition root.
  import { session, checkSaved, openControls, refreshPlayback, STEPS } from "$lib/state/session.svelte";
  import { checkSavedFilter, filterState, closeDetail } from "$lib/state/filter.svelte";
  import { resetCreation } from "$lib/state/creation.svelte";
  import type { Tab } from "$lib/types";
  import NavBar from "$lib/components/NavBar.svelte";
  import TabBar from "$lib/components/TabBar.svelte";
  import DevicesPage from "$lib/components/DevicesPage.svelte";
  import OpenControlsPage from "$lib/components/OpenControlsPage.svelte";
  import SelectFilterPage from "$lib/components/SelectFilterPage.svelte";
  import CreateFilterPage from "$lib/components/CreateFilterPage.svelte";

  let activeTab = $state<Tab>("controls");
  // Manually opened via NavBar's Devices button while a control session is
  // already active -- distinct from session.page, which drives the
  // pre-session landing flow (checking/saved/wizard) on its own.
  let devicesOpen = $state(false);

  // Runs Pair-Verify + bootstraps the control session, then (on success)
  // brings in the other two modules' post-session setup -- mirrors the
  // original single-page version's openControls(), just composed here since
  // that setup now spans three separate state modules.
  async function openControlsFlow() {
    const ok = await openControls();
    if (!ok) return;
    await checkSavedFilter();
    resetCreation();
    devicesOpen = false;
    activeTab = "controls";
  }

  // On mount: check for a saved pairing, and if one exists, connect
  // automatically (VidAngel-style auto-connect) rather than waiting for a
  // manual "Open Controls" tap -- Devices is only shown up front when
  // there's nothing saved yet, or that auto-connect attempt fails.
  $effect(() => {
    (async () => {
      await checkSaved();
      if (session.page === "saved") {
        await openControlsFlow();
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

  let showDevicesPage = $derived(session.page !== "control" || devicesOpen);

  // Purely cosmetic label for the sticky nav bar -- doesn't drive any
  // behavior, just names whichever screen is currently showing.
  let navTitle = $derived.by(() => {
    if (session.page === "checking") return "Family Filter";
    if (session.page === "wizard") {
      if (session.step === "save") return "Save Pairing";
      if (session.step === "done") return "Paired";
      return "Pair an Apple TV";
    }
    if (showDevicesPage) return "Devices";
    if (activeTab === "controls") return "Open Controls";
    if (activeTab === "select-filter") return filterState.detail?.title ?? "Select Filter";
    return "Create Filter";
  });

  // Whether the sticky nav bar's back button should show for the current
  // screen. There's no real history stack -- each screen has exactly one
  // well-defined "back", mirrored by goBack() below.
  let canGoBack = $derived.by(() => {
    if (session.page === "wizard") return session.step !== "companion" || session.savedPairing !== null;
    if (session.page === "control") {
      if (devicesOpen) return true;
      if (activeTab === "select-filter" && filterState.detail) return true;
    }
    return false;
  });

  function goBack() {
    session.error = "";
    if (session.page === "wizard") {
      if (session.step === "companion") {
        if (session.savedPairing) session.page = "saved";
        return;
      }
      const i = STEPS.indexOf(session.step);
      session.step = STEPS[i - 1];
      return;
    }
    if (session.page === "control") {
      if (devicesOpen) {
        devicesOpen = false;
        return;
      }
      if (activeTab === "select-filter" && filterState.detail) {
        closeDetail();
      }
    }
  }
</script>

<div class="phone-shell">
  <main class="canvas">
    <NavBar
      title={navTitle}
      {canGoBack}
      onBack={goBack}
      showDevices={session.page === "control" && !devicesOpen}
      onDevices={() => { devicesOpen = true; }}
    />

    <div class="content" class:with-tabbar={session.page === "control" && !devicesOpen}>
      {#if session.page === "checking"}
        <p class="hint centered">Checking for a saved pairing…</p>
      {:else if showDevicesPage}
        <DevicesPage sessionActive={session.page === "control"} onOpenControls={openControlsFlow} onClose={() => { devicesOpen = false; }} />
      {:else if activeTab === "controls"}
        <OpenControlsPage />
      {:else if activeTab === "select-filter"}
        <SelectFilterPage />
      {:else}
        <CreateFilterPage />
      {/if}
    </div>

    {#if session.page === "control" && !devicesOpen}
      <TabBar active={activeTab} onSelect={(tab) => { activeTab = tab; }} />
    {/if}
  </main>
</div>
