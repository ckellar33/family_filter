<script lang="ts">
  // Pair a new device / verify the existing one -- VidAngel's top-right
  // "Services" entry, renamed to Devices. Two sub-views depending on
  // session.page: "wizard" (discover-and-pair, unchanged from the original
  // single-page flow) or the saved-pairing summary (verify / pair a
  // different device), shown either as the pre-session landing screen or as
  // an overlay reachable via NavBar's Devices button once a session is
  // already active (see `sessionActive`).
  import { untrack } from "svelte";
  import {
    session,
    checkSaved,
    STEPS,
    PROTOCOL_LABEL,
    isProtocol,
    scan,
    pair,
    submitPin,
    skipStep,
    save,
    verifySaved,
  } from "$lib/state/session.svelte";
  import type { Protocol, Step } from "$lib/types";
  import PinKeypad from "$lib/components/PinKeypad.svelte";

  let {
    sessionActive = false,
    onOpenControls,
    onClose,
  }: {
    sessionActive?: boolean;
    onOpenControls: () => void | Promise<void>;
    onClose?: () => void;
  } = $props();

  // One sentence per wizard step, so each screen says what it's for and
  // whether it matters -- previously only the optional-ness was stated.
  const STEP_COPY: Record<Step, { title: string; body: string }> = {
    companion: {
      title: "Find your Apple TV",
      body: "Companion is the required one — it's what lets Family Filter mute and skip. Your Apple TV must be awake and on this Wi-Fi.",
    },
    mrp: {
      title: "Add MRP (optional)",
      body: "MRP reports the live playback position cues are timed against. Skip it if this Apple TV isn't reachable this way.",
    },
    airplay: {
      title: "Add AirPlay (optional)",
      body: "AirPlay is the fallback for playback position. Skip it and Family Filter still mutes and skips — just without a live timeline.",
    },
    save: {
      title: "Save this pairing",
      body: "Credentials get written to pairing.store so the next launch reconnects on its own.",
    },
    done: {
      title: "You're paired",
      body: "Family Filter will reconnect to this Apple TV on its own next time.",
    },
  };

  // On mount: check for a saved pairing.store, and if one exists, connect
  // automatically (VidAngel-style auto-connect) rather than waiting for a
  // manual "Open Controls" tap. This is now the one place in the app that
  // decides connection status -- the root page just lands on Select Filter
  // and leaves connecting up to whoever opens this view (a NavBar tap, or
  // Open Controls' own "connect a device" prompt).
  //
  // Skipped once already connected (session.page === "control"): this
  // effect re-runs every time Devices is opened (it's mounted fresh each
  // time, behind +page.svelte's `{#if devicesOpen}`), and re-checking here
  // would otherwise silently tear down and reconnect an already-active
  // control session -- resetting the loaded filter list, disabled
  // categories/cues, etc. -- just from the user peeking at Devices to check
  // status or pair a second one.
  //
  // The guard reads session.page through `untrack` deliberately -- this
  // effect has to run exactly once per mount (matching what the check +
  // auto-connect itself already relies on: no re-entrant runs while
  // `checkSaved()`/`onOpenControls()` are still in flight and mutating
  // `session.page` themselves). A plain (tracked) read here would make the
  // effect a subscriber of session.page and re-fire on every transition it
  // causes ("checking" -> "saved" -> "control"), triggering an overlapping
  // second check-and-connect attempt each time.
  $effect(() => {
    (async () => {
      if (untrack(() => session.page) === "control") return;
      await checkSaved();
      if (session.page === "saved") {
        await onOpenControls();
      }
    })();
  });

  // Re-scan automatically whenever the wizard lands on a new discovery step.
  $effect(() => {
    if (session.page === "wizard" && isProtocol(session.step)) {
      scan(session.step);
    }
  });
</script>

{#if session.page === "checking"}
  <section class="screen">
    <p class="hint centered">Checking for a saved pairing…</p>
  </section>
{:else if session.page === "wizard"}
  {@const copy = STEP_COPY[session.step]}
  <section class="screen">
    <div class="segmented">
      {#each ["companion", "mrp", "airplay", "save"] as s (s)}
        <div class="segment" class:active={session.step === s} class:done={STEPS.indexOf(session.step) > STEPS.indexOf(s as Step)}>
          {isProtocol(s as Step) ? PROTOCOL_LABEL[s as Protocol] : "Save"}
        </div>
      {/each}
    </div>

    <div>
      <h2 class="empty-title" style="font-size:28px">{copy.title}</h2>
      <p class="empty-body" style="font-size:14px">{copy.body}</p>
    </div>

    {#if session.error && !isProtocol(session.step)}
      <p class="banner error">{session.error}</p>
    {/if}

    {#if isProtocol(session.step)}
      {#if session.awaitingPinFor === session.step}
        <PinKeypad bind:value={session.pin} onSubmit={submitPin} />
      {:else}
        {#if session.error}
          <p class="banner error">
            {session.error}{session.step !== "companion" ? " You can skip this one." : ""}
          </p>
        {/if}

        {#if session.devices.length > 0}
          <ul class="list">
            {#each session.devices as device (device.host + device.port)}
              <li>
                <button class="list-row" onclick={() => pair(session.step as Protocol, device)} disabled={session.pairing}>
                  <span class="device-icon">📺</span>
                  <span class="device-row-text">
                    <span>{device.host}</span>
                    <span class="addr">{device.host}:{device.port}</span>
                  </span>
                  <span class="chevron">›</span>
                </button>
              </li>
            {/each}
          </ul>
        {:else if session.scanning}
          <ul class="list">
            <li class="list-row static" style="justify-content:center; padding:24px 16px">
              <span class="hint centered" style="margin:0">Looking on your network…</span>
            </li>
          </ul>
        {/if}

        <div style="display:flex; gap:10px">
          <button class="btn-secondary" onclick={() => scan(session.step as Protocol)} disabled={session.scanning || session.pairing}>
            {session.scanning ? "Scanning…" : "Scan again"}
          </button>
          {#if session.step !== "companion"}
            <button class="btn-secondary" style="background: var(--grouped-bg); border-color: transparent" onclick={skipStep} disabled={session.pairing}>
              Skip this one
            </button>
          {/if}
        </div>
      {/if}
    {:else if session.step === "save"}
      <button class="btn-primary" onclick={save} disabled={session.pairing}>{session.pairing ? "Saving…" : "Save this pairing"}</button>
    {:else if session.step === "done"}
      <div style="display:flex; flex-direction:column; align-items:center; gap:16px">
        <div
          style="width:88px; height:98px; border-radius:28px 28px 44px 44px; background: var(--accent); color:#fff; display:flex; align-items:center; justify-content:center; font-size:32px"
        >
          ✓
        </div>
        <p class="footnote" style="text-align:center">Saved to <code>pairing.store</code>.</p>
      </div>
      <button class="btn-primary" onclick={onOpenControls}>Open controls</button>
    {/if}
  </section>
{:else if session.savedPairing}
  <section class="screen">
    <div class="device-card">
      <div class="device-card-head">
        <span class="device-icon" style="background: rgba(245,241,234,.12); width:44px; height:44px; border-radius:14px; font-size:17px">📺</span>
        <div style="flex:1; min-width:0">
          <p class="name">{session.savedPairing.host}</p>
          <p class="addr">{session.savedPairing.host}:{session.savedPairing.port}</p>
        </div>
        <span class="shield" data-state={session.verifyResult === "ok" ? "on" : "off"}>
          {session.verifying ? "CHECKING" : session.verifyResult === "ok" ? "VERIFIED" : "SAVED"}
        </span>
      </div>
      <div class="protocol-tiles">
        <div class="protocol-tile">
          <span class="name">Companion</span>
          <span class="state paired">Paired</span>
        </div>
        <div class="protocol-tile">
          <span class="name">MRP</span>
          <span class="state" class:paired={session.savedPairing.has_mrp}>{session.savedPairing.has_mrp ? "Paired" : "Not paired"}</span>
        </div>
        <div class="protocol-tile">
          <span class="name">AirPlay</span>
          <span class="state" class:paired={session.savedPairing.has_airplay}>
            {session.savedPairing.has_airplay ? "Paired" : "Not paired"}
          </span>
        </div>
      </div>
    </div>

    <p class="hint">
      Companion is what lets Family Filter mute and skip. MRP or AirPlay adds the live playback position the cues are timed against.
    </p>

    {#if session.connecting}
      <p class="hint centered">Connecting…</p>
    {:else if session.verifyResult === "failed"}
      <p class="banner error">{session.verifyError}</p>
    {:else if session.error}
      <p class="banner error">{session.error}</p>
    {/if}

    <div class="stack">
      {#if sessionActive}
        <button class="btn-primary" onclick={onClose} disabled={session.verifying}>Done</button>
      {:else}
        <button class="btn-primary" onclick={onOpenControls} disabled={session.verifying || session.connecting}>
          {session.connecting ? "Connecting…" : "Open controls"}
        </button>
      {/if}
      <button class="btn-secondary" onclick={verifySaved} disabled={session.verifying || session.connecting}>
        {session.verifying ? "Verifying…" : "Verify this device"}
      </button>
      <button class="btn-secondary" onclick={() => { session.page = "wizard"; }} disabled={session.verifying || session.connecting}>
        Pair a new device
      </button>
    </div>
  </section>
{/if}
