<script lang="ts">
  // Pair a new device / verify the existing one -- VidAngel's top-right
  // "Services" entry, renamed to Devices. Two sub-views depending on
  // session.page: "wizard" (discover-and-pair, unchanged from the original
  // single-page flow) or the saved-pairing summary (verify / pair a
  // different device), shown either as the pre-session landing screen or as
  // an overlay reachable via NavBar's Devices button once a session is
  // already active (see `sessionActive`).
  import { session, STEPS, PROTOCOL_LABEL, isProtocol, scan, pair, submitPin, skipStep, save, verifySaved } from "$lib/state/session.svelte";
  import type { Protocol, Step } from "$lib/types";

  let {
    sessionActive = false,
    onOpenControls,
    onClose,
  }: {
    sessionActive?: boolean;
    onOpenControls: () => void | Promise<void>;
    onClose?: () => void;
  } = $props();

  // Re-scan automatically whenever the wizard lands on a new discovery step.
  $effect(() => {
    if (session.page === "wizard" && isProtocol(session.step)) {
      scan(session.step);
    }
  });
</script>

{#if session.page === "wizard"}
  <section class="screen">
    <div class="segmented">
      {#each ["companion", "mrp", "airplay", "save"] as s (s)}
        <div class="segment" class:active={session.step === s} class:done={STEPS.indexOf(session.step) > STEPS.indexOf(s as Step)}>
          {isProtocol(s as Step) ? PROTOCOL_LABEL[s as Protocol] : "Save"}
        </div>
      {/each}
    </div>

    {#if session.error}
      <p class="banner error">{session.error}</p>
    {/if}

    {#if isProtocol(session.step)}
      <p class="section-header">{PROTOCOL_LABEL[session.step]} pairing{session.step !== "companion" ? " (optional)" : ""}</p>
      {#if session.step !== "companion"}
        <p class="hint">Needed only for live playback position. Skip if this Apple TV isn't reachable over {PROTOCOL_LABEL[session.step]}.</p>
      {/if}

      {#if session.awaitingPinFor === session.step}
        <form onsubmit={(e) => { e.preventDefault(); submitPin(); }}>
          <p class="hint centered">Enter the PIN shown on your Apple TV:</p>
          <input class="pin-input" inputmode="numeric" autocomplete="one-time-code" bind:value={session.pin} placeholder="0000" />
          <button type="submit" class="btn-primary" disabled={!session.pin}>Submit</button>
        </form>
      {:else}
        <div class="stack">
          <button class="btn-secondary" onclick={() => scan(session.step as Protocol)} disabled={session.scanning || session.pairing}>
            {session.scanning ? "Scanning…" : "Rescan"}
          </button>
          {#if session.step !== "companion"}
            <button class="btn-secondary" onclick={skipStep} disabled={session.pairing}>Skip</button>
          {/if}
        </div>

        {#if session.devices.length > 0}
          <ul class="list">
            {#each session.devices as device (device.host + device.port)}
              <li>
                <button class="list-row" onclick={() => pair(session.step as Protocol, device)} disabled={session.pairing}>
                  <span>{device.host}:{device.port}</span>
                  <span class="chevron">›</span>
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      {/if}
    {:else if session.step === "save"}
      <p class="section-header">Save pairing</p>
      <p class="hint">Ready to save credentials to <code>pairing.store</code>.</p>
      <button class="btn-primary" onclick={save} disabled={session.pairing}>{session.pairing ? "Saving…" : "Save"}</button>
    {:else if session.step === "done"}
      <p class="section-header">✅ Paired</p>
      <p class="hint">Credentials saved. This Apple TV is ready to control.</p>
      <button class="btn-primary" onclick={onOpenControls}>Open Controls</button>
    {/if}
  </section>
{:else if session.savedPairing}
  <section class="screen">
    <p class="section-header">Device</p>
    <ul class="list">
      <li class="list-row static">
        <code>{session.savedPairing.host}:{session.savedPairing.port}</code>
      </li>
      <li class="list-row static">
        <span>MRP</span>
        <span class="value">{session.savedPairing.has_mrp ? "Paired" : "Not paired"}</span>
      </li>
      <li class="list-row static">
        <span>AirPlay</span>
        <span class="value">{session.savedPairing.has_airplay ? "Paired" : "Not paired"}</span>
      </li>
    </ul>

    {#if session.verifyResult === "ok"}
      <p class="banner success">✅ Verified — this pairing is still valid.</p>
    {:else if session.verifyResult === "failed"}
      <p class="banner error">{session.verifyError}</p>
    {:else if session.error}
      <p class="banner error">{session.error}</p>
    {/if}

    <div class="stack">
      {#if sessionActive}
        <button class="btn-primary" onclick={onClose} disabled={session.verifying}>Done</button>
      {:else}
        <button class="btn-primary" onclick={onOpenControls} disabled={session.verifying}>Open Controls</button>
      {/if}
      <button class="btn-secondary" onclick={verifySaved} disabled={session.verifying}>
        {session.verifying ? "Verifying…" : "Verify Existing Device"}
      </button>
      <button class="btn-secondary" onclick={() => { session.page = "wizard"; }} disabled={session.verifying}>Pair a New Device</button>
    </div>
  </section>
{/if}
