<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";

  interface Device {
    host: string;
    port: number;
  }

  interface SavedPairingInfo {
    host: string;
    port: number;
    has_mrp: boolean;
    has_airplay: boolean;
  }

  interface ControlInfo {
    has_live: boolean;
  }

  interface PlaybackStatus {
    title: string | null;
    position: number | null;
    duration: number | null;
    playback_state: string;
  }

  type Protocol = "companion" | "mrp" | "airplay";
  type Step = Protocol | "save" | "done";

  // Companion is required (it's what unlocks mute/skip control); MRP and
  // AirPlay are each their own optional pairing ceremony against their own
  // discovered device, needed only for live playback position. Mirrors
  // libs/appletv-cli's pair_flow().
  const STEPS: Step[] = ["companion", "mrp", "airplay", "save", "done"];
  const PROTOCOL_LABEL: Record<Protocol, string> = {
    companion: "Companion",
    mrp: "MRP",
    airplay: "AirPlay",
  };

  // Top-level page: "checking" while we ask the backend for a saved
  // pairing.store on mount, "saved" if one was found (offer to verify it
  // instead of re-pairing from scratch), "wizard" for the discover-and-pair
  // flow -- either because there was nothing saved, or the user chose to
  // pair a different device anyway -- and "control" once a control session
  // is open (mute/unmute, skip, now playing).
  type Page = "checking" | "saved" | "wizard" | "control";
  let page = $state<Page>("checking");
  let savedPairing = $state<SavedPairingInfo | null>(null);
  let verifying = $state(false);
  let verifyResult = $state<"ok" | "failed" | null>(null);
  let verifyError = $state("");

  let hasLive = $state(false);
  let playback = $state<PlaybackStatus | null>(null);
  let controlBusy = $state(false);
  let controlError = $state("");

  let step = $state<Step>("companion");
  let devices = $state<Device[]>([]);
  let scanning = $state(false);
  let pairing = $state(false);
  let error = $state("");

  // Set once the backend's `pin-requested` event fires for the protocol
  // currently being paired -- i.e. the on-screen code is now showing on the
  // Apple TV and the backend is awaiting `submit_pin`.
  let awaitingPinFor = $state<Protocol | null>(null);
  let pin = $state("");

  function isProtocol(s: Step): s is Protocol {
    return s === "companion" || s === "mrp" || s === "airplay";
  }

  // Runs once on mount (no reactive dependencies) to decide the starting
  // page: straight into the wizard if there's nothing saved yet, or the
  // saved-pairing screen if libs/appletv-cli (or an earlier run of this
  // app) already produced a pairing.store.
  $effect(() => {
    checkSaved();
  });

  async function checkSaved() {
    try {
      const found = await invoke<SavedPairingInfo | null>("check_saved_pairing");
      if (found) {
        savedPairing = found;
        page = "saved";
      } else {
        page = "wizard";
      }
    } catch (e) {
      // Backend couldn't even check -- fall through to the normal pairing
      // wizard rather than get stuck on "checking...".
      error = String(e);
      page = "wizard";
    }
  }

  async function verifySaved() {
    verifying = true;
    verifyResult = null;
    verifyError = "";
    try {
      await invoke("verify_saved_pairing");
      verifyResult = "ok";
    } catch (e) {
      verifyResult = "failed";
      verifyError = String(e);
    } finally {
      verifying = false;
    }
  }

  // Runs Pair-Verify + bootstraps a Companion control session (plus a live
  // MRP/AirPlay session if one was paired), then switches to the control
  // page. Available from both the saved-pairing screen and right after a
  // fresh pairing finishes.
  async function openControls() {
    error = "";
    try {
      const info = await invoke<ControlInfo>("start_control_session");
      hasLive = info.has_live;
      playback = null;
      controlError = "";
      page = "control";
    } catch (e) {
      error = String(e);
    }
  }

  async function doSkip(seconds: number) {
    controlBusy = true;
    controlError = "";
    try {
      await invoke("control_skip", { seconds });
      // Backend actively re-fetches on every control_playback_status call
      // now, but that's on the next 1s poll tick -- fetch immediately too
      // so the display catches up to the skip right away.
      await refreshPlayback();
    } catch (e) {
      controlError = String(e);
    } finally {
      controlBusy = false;
    }
  }

  async function doMute() {
    controlBusy = true;
    controlError = "";
    try {
      await invoke("control_mute");
    } catch (e) {
      controlError = String(e);
    } finally {
      controlBusy = false;
    }
  }

  async function doUnmute() {
    controlBusy = true;
    controlError = "";
    try {
      await invoke("control_unmute");
    } catch (e) {
      controlError = String(e);
    } finally {
      controlBusy = false;
    }
  }

  // Fetches one playback snapshot -- errors propagate to the caller rather
  // than being handled here, since callers (doSkip, the periodic poll)
  // each have their own busy/error state to manage around it.
  async function refreshPlayback() {
    playback = await invoke<PlaybackStatus | null>("control_playback_status");
  }

  // Polls now-playing status once a second while the control page is open
  // and a live (MRP/AirPlay) transport is available -- there's nothing to
  // poll otherwise, since title/position only ever come from that
  // transport, not Companion. The backend actively re-fetches on every
  // call (see control_playback_status), so this alone keeps the display
  // in sync with skips and remote-triggered pauses/seeks automatically.
  $effect(() => {
    if (page !== "control" || !hasLive) return;
    const id = setInterval(async () => {
      try {
        await refreshPlayback();
      } catch (e) {
        controlError = String(e);
      }
    }, 1000);
    return () => clearInterval(id);
  });

  function fmtTime(seconds: number | null | undefined): string {
    if (seconds == null) return "--:--";
    const total = Math.max(0, Math.round(seconds));
    const m = Math.floor(total / 60);
    const s = total % 60;
    return `${m}:${s.toString().padStart(2, "0")}`;
  }

  async function scan(protocol: Protocol) {
    scanning = true;
    error = "";
    devices = [];
    try {
      devices = await invoke<Device[]>("discover_devices", { protocol });
      if (devices.length === 0) {
        error = `No ${PROTOCOL_LABEL[protocol]} devices found on the network.`;
      }
    } catch (e) {
      error = String(e);
    } finally {
      scanning = false;
    }
  }

  // Re-scan automatically whenever the wizard lands on a new discovery
  // step -- but only once we're actually on the wizard page (not while
  // still checking for, or looking at, a saved pairing).
  $effect(() => {
    if (page === "wizard" && isProtocol(step)) {
      scan(step);
    }
  });

  async function pair(protocol: Protocol, device: Device) {
    pairing = true;
    error = "";
    let unlisten: UnlistenFn | undefined;
    try {
      // Must be listening *before* invoking -- the backend can fire
      // pin-requested partway through the still-pending invoke() call.
      unlisten = await listen<Protocol>("pin-requested", (event) => {
        if (event.payload === protocol) {
          awaitingPinFor = protocol;
        }
      });
      await invoke(`pair_${protocol}`, { host: device.host, port: device.port });
      advance();
    } catch (e) {
      error = String(e);
    } finally {
      unlisten?.();
      awaitingPinFor = null;
      pin = "";
      pairing = false;
    }
  }

  async function submitPin() {
    if (!awaitingPinFor) return;
    try {
      await invoke("submit_pin", { protocol: awaitingPinFor, pin });
    } catch (e) {
      error = String(e);
    }
  }

  function advance() {
    const i = STEPS.indexOf(step);
    step = STEPS[i + 1];
  }

  function skip() {
    error = "";
    advance();
  }

  async function save() {
    pairing = true;
    error = "";
    try {
      await invoke("finish_pairing");
      step = "done";
    } catch (e) {
      error = String(e);
    } finally {
      pairing = false;
    }
  }
</script>

<main class="container">
  <h1>Pair an Apple TV</h1>

  {#if page === "checking"}
    <p>Checking for a saved pairing…</p>
  {:else if page === "saved" && savedPairing}
    <section>
      <h2>Saved Apple TV found</h2>
      <p><code>{savedPairing.host}:{savedPairing.port}</code></p>
      <p class="hint">
        MRP: {savedPairing.has_mrp ? "paired" : "not paired"} · AirPlay: {savedPairing.has_airplay ? "paired" : "not paired"}
      </p>

      {#if verifyResult === "ok"}
        <p class="success">✅ Verified — this pairing is still valid.</p>
      {:else if verifyResult === "failed"}
        <p class="error">{verifyError}</p>
      {/if}

      <div class="row">
        <button onclick={verifySaved} disabled={verifying}>{verifying ? "Verifying…" : "Verify"}</button>
        <button onclick={openControls} disabled={verifying}>Open controls</button>
        <button onclick={() => { page = "wizard"; }} disabled={verifying}>Pair a different device</button>
      </div>
    </section>
  {:else if page === "control"}
    <section>
      <h2>Control</h2>
      {#if controlError}
        <p class="error">{controlError}</p>
      {/if}

      {#if hasLive}
        <div class="now-playing">
          <p class="title">{playback?.title ?? "(nothing playing)"}</p>
          <p class="position">
            {fmtTime(playback?.position)} / {fmtTime(playback?.duration)}
            {#if playback}· {playback.playback_state}{/if}
          </p>
        </div>
      {:else}
        <p class="hint">Pair MRP or AirPlay (from the pairing wizard) to unlock mute/unmute and playback info.</p>
      {/if}

      <div class="row">
        <button onclick={() => doSkip(-15)} disabled={controlBusy}>⏪ 15s</button>
        <button onclick={() => doSkip(15)} disabled={controlBusy}>15s ⏩</button>
      </div>

      {#if hasLive}
        <div class="row">
          <button onclick={doMute} disabled={controlBusy}>🔇 Mute</button>
          <button onclick={doUnmute} disabled={controlBusy}>🔊 Unmute</button>
        </div>
      {/if}
    </section>
  {:else}
    <ol class="steps">
      {#each ["companion", "mrp", "airplay", "save"] as s (s)}
        <li class:active={step === s} class:done={STEPS.indexOf(step) > STEPS.indexOf(s as Step)}>
          {isProtocol(s as Step) ? PROTOCOL_LABEL[s as Protocol] : "Save"}
        </li>
      {/each}
    </ol>

    {#if error}
      <p class="error">{error}</p>
    {/if}

    {#if isProtocol(step)}
    <section>
      <h2>{PROTOCOL_LABEL[step]} pairing{step !== "companion" ? " (optional)" : ""}</h2>
      {#if step !== "companion"}
        <p class="hint">Needed only for live playback position. Skip if this Apple TV isn't reachable over {PROTOCOL_LABEL[step]}.</p>
      {/if}

      {#if awaitingPinFor === step}
        <form class="row" onsubmit={(e) => { e.preventDefault(); submitPin(); }}>
          <p>Enter the PIN shown on your Apple TV:</p>
          <input inputmode="numeric" autocomplete="one-time-code" bind:value={pin} placeholder="0000" />
          <button type="submit" disabled={!pin}>Submit</button>
        </form>
      {:else}
        <div class="row">
          <button onclick={() => scan(step as Protocol)} disabled={scanning || pairing}>
            {scanning ? "Scanning…" : "Rescan"}
          </button>
          {#if step !== "companion"}
            <button onclick={skip} disabled={pairing}>Skip</button>
          {/if}
        </div>

        {#if devices.length > 0}
          <ul class="devices">
            {#each devices as device (device.host + device.port)}
              <li>
                <button onclick={() => pair(step as Protocol, device)} disabled={pairing}>
                  {device.host}:{device.port}
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      {/if}
    </section>
  {:else if step === "save"}
    <section>
      <h2>Save pairing</h2>
      <p>Ready to save credentials to <code>pairing.store</code>.</p>
      <button onclick={save} disabled={pairing}>{pairing ? "Saving…" : "Save"}</button>
    </section>
  {:else if step === "done"}
    <section>
      <h2>✅ Paired</h2>
      <p>Credentials saved. This Apple TV is ready to control.</p>
      <button onclick={openControls}>Open controls</button>
    </section>
    {/if}
  {/if}

</main>

<style>
:root {
  font-family: Inter, Avenir, Helvetica, Arial, sans-serif;
  font-size: 16px;
  line-height: 24px;
  font-weight: 400;

  color: #0f0f0f;
  background-color: #f6f6f6;

  font-synthesis: none;
  text-rendering: optimizeLegibility;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  -webkit-text-size-adjust: 100%;
}

.container {
  margin: 0 auto;
  max-width: 32rem;
  padding: 3rem 1.5rem;
}

h1 {
  text-align: center;
}

.steps {
  display: flex;
  justify-content: space-between;
  list-style: none;
  padding: 0;
  margin: 2rem 0;
  font-size: 0.85em;
}

.steps li {
  flex: 1;
  text-align: center;
  padding-bottom: 0.5em;
  border-bottom: 3px solid #d8d8d8;
  color: #888;
}

.steps li.done {
  border-color: #396cd8;
  color: #396cd8;
}

.steps li.active {
  border-color: #24c8db;
  color: #0f0f0f;
  font-weight: 600;
}

.error {
  color: #c0392b;
  background: #fdecea;
  border-radius: 8px;
  padding: 0.75em 1em;
}

.success {
  color: #1e7e34;
  background: #e6f6ea;
  border-radius: 8px;
  padding: 0.75em 1em;
}

.hint {
  color: #666;
  font-size: 0.9em;
}

.now-playing {
  text-align: center;
  margin: 1.5em 0;
}

.now-playing .title {
  font-size: 1.2em;
  font-weight: 600;
  margin: 0 0 0.25em;
}

.now-playing .position {
  color: #666;
  font-variant-numeric: tabular-nums;
  margin: 0;
}

.row {
  display: flex;
  gap: 0.5em;
  align-items: center;
  margin: 1em 0;
}

.devices {
  list-style: none;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0.5em;
}

.devices button {
  width: 100%;
  text-align: left;
}

input,
button {
  border-radius: 8px;
  border: 1px solid transparent;
  padding: 0.6em 1.2em;
  font-size: 1em;
  font-weight: 500;
  font-family: inherit;
  color: #0f0f0f;
  background-color: #ffffff;
  transition: border-color 0.25s;
  box-shadow: 0 2px 2px rgba(0, 0, 0, 0.2);
}

button {
  cursor: pointer;
}

button:hover:not(:disabled) {
  border-color: #396cd8;
}

button:disabled {
  opacity: 0.5;
  cursor: default;
}

input,
button {
  outline: none;
}

@media (prefers-color-scheme: dark) {
  :root {
    color: #f6f6f6;
    background-color: #2f2f2f;
  }

  .steps li {
    border-color: #444;
  }

  .error {
    background: #4a1f1c;
    color: #ff8a80;
  }

  .success {
    background: #1c3a24;
    color: #8fe0a4;
  }

  .hint {
    color: #aaa;
  }

  .now-playing .position {
    color: #aaa;
  }

  input,
  button {
    color: #ffffff;
    background-color: #0f0f0f98;
  }
}
</style>
