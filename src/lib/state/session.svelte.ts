// Pairing (Devices page) + control-session state: everything that used to
// live at the top of the single +page.svelte script, now importable from
// any component. An exported `const session = $state({...})` object rather
// than several exported `let`s -- Svelte only allows reassigning an
// imported binding from within the module that declared it, but mutating a
// *property* of an exported reactive object works fine from anywhere, which
// is what every setter function below (and every consuming component) does.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ControlInfo, Device, PlaybackStatus, Protocol, RemoteButton, SavedPairingInfo, Step } from "$lib/types";

// Companion is required (it's what unlocks mute/skip control); MRP and
// AirPlay are each their own optional pairing ceremony against their own
// discovered device, needed only for live playback position. Mirrors
// libs/appletv-cli's pair_flow().
export const STEPS: Step[] = ["companion", "mrp", "airplay", "save", "done"];
export const PROTOCOL_LABEL: Record<Protocol, string> = {
  companion: "Companion",
  mrp: "MRP",
  airplay: "AirPlay",
};

export function isProtocol(s: Step): s is Protocol {
  return s === "companion" || s === "mrp" || s === "airplay";
}

// Top-level pairing state: "checking" while we ask the backend for a saved
// pairing.store on mount, "saved" if one was found (offer to verify it
// instead of re-pairing from scratch), "wizard" for the discover-and-pair
// flow -- either because there was nothing saved, or the user chose to pair
// a different device anyway -- and "control" once a control session is open
// (mute/unmute, skip, now playing). Unlike the old single-page version,
// "control" no longer means one specific screen -- see +page.svelte's
// devicesOpen/activeTab for what's actually shown once a session is active.
export type Page = "checking" | "saved" | "wizard" | "control";

export const session = $state({
  page: "checking" as Page,
  savedPairing: null as SavedPairingInfo | null,
  verifying: false,
  verifyResult: null as "ok" | "failed" | null,
  verifyError: "",

  // True for the duration of one openControls() call -- covers both the
  // auto-connect attempt on launch and a manual "Open Controls" tap. The
  // underlying connect can take several seconds (or, now bounded, time out)
  // against a slow/unreachable Apple TV; without this the saved-pairing
  // screen looked identical whether nothing had been tried yet or an
  // attempt was quietly still in flight.
  connecting: false,

  hasLive: false,
  playback: null as PlaybackStatus | null,
  controlBusy: false,
  controlError: "",

  step: "companion" as Step,
  devices: [] as Device[],
  scanning: false,
  pairing: false,
  error: "",

  // Set once the backend's `pin-requested` event fires for the protocol
  // currently being paired -- i.e. the on-screen code is now showing on the
  // Apple TV and the backend is awaiting `submit_pin`.
  awaitingPinFor: null as Protocol | null,
  pin: "",
});

// Runs once on mount to decide the starting page: straight into the wizard
// if there's nothing saved yet, or the saved-pairing screen if
// libs/appletv-cli (or an earlier run of this app) already produced a
// pairing.store. Callers decide what to do next (see +page.svelte, which
// auto-runs openControls() when this finds a saved pairing).
export async function checkSaved() {
  try {
    const found = await invoke<SavedPairingInfo | null>("check_saved_pairing");
    if (found) {
      session.savedPairing = found;
      session.page = "saved";
    } else {
      session.page = "wizard";
    }
  } catch (e) {
    // Backend couldn't even check -- fall through to the normal pairing
    // wizard rather than get stuck on "checking...".
    session.error = String(e);
    session.page = "wizard";
  }
}

export async function verifySaved() {
  session.verifying = true;
  session.verifyResult = null;
  session.verifyError = "";
  try {
    await invoke("verify_saved_pairing");
    session.verifyResult = "ok";
  } catch (e) {
    session.verifyResult = "failed";
    session.verifyError = String(e);
  } finally {
    session.verifying = false;
  }
}

// Runs Pair-Verify + bootstraps a Companion control session (plus a live
// MRP/AirPlay session if one was paired). Available from the saved-pairing
// screen, right after a fresh pairing finishes, or automatically on launch
// (see +page.svelte). Returns whether it succeeded so callers can decide
// what to show next -- doesn't touch filter/creation state itself (that's
// each module's own job; +page.svelte composes them after this resolves).
export async function openControls(): Promise<boolean> {
  session.error = "";
  session.connecting = true;
  try {
    const info = await invoke<ControlInfo>("start_control_session");
    session.hasLive = info.has_live;
    session.playback = null;
    session.controlError = "";
    session.page = "control";
    return true;
  } catch (e) {
    session.error = String(e);
    return false;
  } finally {
    session.connecting = false;
  }
}

export async function doSkip(seconds: number) {
  session.controlBusy = true;
  session.controlError = "";
  try {
    await invoke("control_skip", { seconds });
    // Backend actively re-fetches on every control_playback_status call
    // now, but that's on the next 1s poll tick -- fetch immediately too so
    // the display catches up to the skip right away.
    await refreshPlayback();
  } catch (e) {
    session.controlError = String(e);
  } finally {
    session.controlBusy = false;
  }
}

// Presses one Siri Remote button (arrows/Select/Menu/Home/Play-Pause) via
// Companion -- see control::control_button. Refreshes playback afterward,
// same as doSkip, since Play/Pause (and a Menu/Home that backs out of
// what's playing) can change playback_state right away.
export async function doButton(button: RemoteButton) {
  session.controlBusy = true;
  session.controlError = "";
  try {
    await invoke("control_button", { button });
    await refreshPlayback();
  } catch (e) {
    session.controlError = String(e);
  } finally {
    session.controlBusy = false;
  }
}

export async function doMute() {
  session.controlBusy = true;
  session.controlError = "";
  try {
    await invoke("control_mute");
  } catch (e) {
    session.controlError = String(e);
  } finally {
    session.controlBusy = false;
  }
}

export async function doUnmute() {
  session.controlBusy = true;
  session.controlError = "";
  try {
    await invoke("control_unmute");
  } catch (e) {
    session.controlError = String(e);
  } finally {
    session.controlBusy = false;
  }
}

// Fetches one playback snapshot -- errors propagate to the caller rather
// than being handled here, since callers (doSkip, the periodic poll) each
// have their own busy/error state to manage around it.
export async function refreshPlayback() {
  session.playback = await invoke<PlaybackStatus | null>("control_playback_status");
}

export async function scan(protocol: Protocol) {
  session.scanning = true;
  session.error = "";
  session.devices = [];
  try {
    session.devices = await invoke<Device[]>("discover_devices", { protocol });
    if (session.devices.length === 0) {
      session.error = `No ${PROTOCOL_LABEL[protocol]} devices found on the network.`;
    }
  } catch (e) {
    session.error = String(e);
  } finally {
    session.scanning = false;
  }
}

export async function pair(protocol: Protocol, device: Device) {
  session.pairing = true;
  session.error = "";
  let unlisten: UnlistenFn | undefined;
  try {
    // Must be listening *before* invoking -- the backend can fire
    // pin-requested partway through the still-pending invoke() call.
    unlisten = await listen<Protocol>("pin-requested", (event) => {
      if (event.payload === protocol) {
        session.awaitingPinFor = protocol;
      }
    });
    await invoke(`pair_${protocol}`, { host: device.host, port: device.port });
    advance();
  } catch (e) {
    session.error = String(e);
  } finally {
    unlisten?.();
    session.awaitingPinFor = null;
    session.pin = "";
    session.pairing = false;
  }
}

export async function submitPin() {
  if (!session.awaitingPinFor) return;
  try {
    await invoke("submit_pin", { protocol: session.awaitingPinFor, pin: session.pin });
  } catch (e) {
    session.error = String(e);
  }
}

export function advance() {
  const i = STEPS.indexOf(session.step);
  session.step = STEPS[i + 1];
}

export function skipStep() {
  session.error = "";
  advance();
}

export async function save() {
  session.pairing = true;
  session.error = "";
  try {
    await invoke("finish_pairing");
    session.step = "done";
  } catch (e) {
    session.error = String(e);
  } finally {
    session.pairing = false;
  }
}
