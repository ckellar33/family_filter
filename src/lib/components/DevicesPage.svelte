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
    deleteDevice,
    startPairingWizard,
  } from "$lib/state/session.svelte";
  import type { Protocol, Step } from "$lib/types";
  import PinKeypad from "$lib/components/PinKeypad.svelte";

  let {
    sessionActive = false,
    onOpenControls,
    onClose,
  }: {
    sessionActive?: boolean;
    onOpenControls: (id: string) => void | Promise<void>;
    onClose?: () => void;
  } = $props();

  // Per-card "remove this device?" confirmation -- a bare tap deletes
  // nothing; a second tap on the same card within the confirm state does.
  // Reset on mount so reopening Devices never lands mid-confirm.
  let confirmDeleteId = $state<string | null>(null);

  function requestDelete(id: string) {
    if (confirmDeleteId === id) {
      confirmDeleteId = null;
      deleteDevice(id);
    } else {
      confirmDeleteId = id;
    }
  }

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
      body: "Give it a name you'll recognize -- Family Filter can remember more than one Apple TV, and you'll pick between them later from Devices.",
    },
    done: {
      title: "You're paired",
      body: "Family Filter will offer to reconnect to this Apple TV on its own next time.",
    },
  };

  // On mount: check for saved devices, and if the last one connected to is
  // still among them, connect to it automatically (VidAngel-style
  // auto-connect) rather than waiting for a manual tap. Every *other* saved
  // device stays one tap away in the list below -- that's this feature's
  // "switch device" option, not a separate screen. +page.svelte already
  // makes this same attempt once, quietly, on app launch, so by the time
  // this component ever mounts (a NavBar tap, or Open Controls' own
  // "connect a device" prompt) it's usually just re-confirming an existing
  // session -- the guard right below turns that into a no-op. Where this
  // copy still earns its keep is as a retry: if the launch attempt failed
  // (device asleep/off Wi-Fi) or there was nothing to reconnect to yet
  // (paired later, mid-session), opening Devices tries again.
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
      confirmDeleteId = null;
      await checkSaved();
      if (session.page === "saved" && session.lastDeviceId && session.savedDevices.some((d) => d.id === session.lastDeviceId)) {
        await onOpenControls(session.lastDeviceId);
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
    <p class="hint centered">Checking for saved devices…</p>
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
      <div class="stack">
        <label class="footnote" for="device-name" style="display:block; margin-bottom:6px">Name this device</label>
        <input
          id="device-name"
          class="field"
          type="text"
          bind:value={session.deviceName}
          placeholder={session.pairedHost}
          maxlength="60"
        />
      </div>
      <button class="btn-primary" onclick={() => save(session.deviceName)} disabled={session.pairing}>
        {session.pairing ? "Saving…" : "Save this pairing"}
      </button>
    {:else if session.step === "done"}
      <div style="display:flex; flex-direction:column; align-items:center; gap:16px">
        <div
          style="width:88px; height:98px; border-radius:28px 28px 44px 44px; background: var(--accent); color:#fff; display:flex; align-items:center; justify-content:center; font-size:32px"
        >
          ✓
        </div>
        <p class="footnote" style="text-align:center">Saved as “{session.deviceName || session.pairedHost}”.</p>
      </div>
      <button class="btn-primary" onclick={() => session.newDeviceId && onOpenControls(session.newDeviceId)}>Open controls</button>
    {/if}
  </section>
{:else if session.savedDevices.length > 0}
  <section class="screen">
    <p class="hint">
      Companion is what lets Family Filter mute and skip. MRP or AirPlay adds the live playback position the cues are timed against.
    </p>

    {#if session.error}
      <p class="banner error">{session.error}</p>
    {/if}

    <div style="display:flex; flex-direction:column; gap:12px">
      {#each session.savedDevices as device (device.id)}
        {@const isActive = sessionActive && session.activeDevice?.id === device.id}
        {@const isConnecting = session.connectingId === device.id}
        {@const isVerifying = session.verifyingId === device.id}
        {@const justVerified = session.verifiedId === device.id}
        <div class="device-card">
          <div class="device-card-head">
            <span class="device-icon" style="background: rgba(245,241,234,.12); width:44px; height:44px; border-radius:14px; font-size:17px">📺</span>
            <div style="flex:1; min-width:0">
              <p class="name">{device.name}</p>
              <p class="addr">{device.host}:{device.port}</p>
            </div>
            <span class="shield" data-state={isActive || (justVerified && session.verifyResult === "ok") ? "on" : "off"}>
              {isVerifying
                ? "CHECKING"
                : isActive
                  ? "CONNECTED"
                  : justVerified && session.verifyResult === "ok"
                    ? "VERIFIED"
                    : device.id === session.lastDeviceId
                      ? "LAST USED"
                      : "SAVED"}
            </span>
          </div>
          <div class="protocol-tiles">
            <div class="protocol-tile">
              <span class="name">Companion</span>
              <span class="state paired">Paired</span>
            </div>
            <div class="protocol-tile">
              <span class="name">MRP</span>
              <span class="state" class:paired={device.has_mrp}>{device.has_mrp ? "Paired" : "Not paired"}</span>
            </div>
            <div class="protocol-tile">
              <span class="name">AirPlay</span>
              <span class="state" class:paired={device.has_airplay}>{device.has_airplay ? "Paired" : "Not paired"}</span>
            </div>
          </div>

          {#if justVerified && session.verifyResult === "failed"}
            <p class="banner error" style="margin:0">{session.verifyError}</p>
          {/if}

          <div class="stack">
            {#if isActive}
              <p class="hint centered" style="margin:0">This is the connected device.</p>
            {:else}
              <button class="btn-primary" onclick={() => onOpenControls(device.id)} disabled={session.connecting || session.verifying}>
                {isConnecting ? "Connecting…" : sessionActive ? "Switch to this device" : "Open controls"}
              </button>
            {/if}
            <div style="display:flex; gap:10px">
              <button class="btn-secondary" onclick={() => verifySaved(device.id)} disabled={session.connecting || session.verifying}>
                {isVerifying ? "Verifying…" : "Verify"}
              </button>
              <button
                class="btn-secondary"
                style={confirmDeleteId === device.id ? "color: var(--destructive); border-color: var(--error-line)" : ""}
                onclick={() => requestDelete(device.id)}
                disabled={session.connecting || session.verifying}
              >
                {confirmDeleteId === device.id ? "Tap again to remove" : "Remove"}
              </button>
            </div>
          </div>
        </div>
      {/each}
    </div>

    <div class="stack">
      {#if sessionActive}
        <button class="btn-primary" onclick={onClose}>Done</button>
      {/if}
      <button class="btn-secondary" onclick={startPairingWizard} disabled={session.verifying || session.connecting}>
        Pair a new device
      </button>
    </div>
  </section>
{/if}
