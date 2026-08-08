<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";

  interface Device {
    host: string;
    port: number;
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

  // Re-scan automatically whenever the wizard lands on a new discovery step.
  $effect(() => {
    if (isProtocol(step)) {
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
    </section>
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

.hint {
  color: #666;
  font-size: 0.9em;
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

  .hint {
    color: #aaa;
  }

  input,
  button {
    color: #ffffff;
    background-color: #0f0f0f98;
  }
}
</style>
