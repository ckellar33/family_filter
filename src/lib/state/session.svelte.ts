// Pairing (Devices page) + control-session state: everything that used to
// live at the top of the single +page.svelte script, now importable from
// any component. An exported `const session = $state({...})` object rather
// than several exported `let`s -- Svelte only allows reassigning an
// imported binding from within the module that declared it, but mutating a
// *property* of an exported reactive object works fine from anywhere, which
// is what every setter function below (and every consuming component) does.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ControlInfo, Device, PlaybackStatus, Protocol, RemoteButton, SavedDeviceInfo, Step } from "$lib/types";

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

// Top-level pairing state: "checking" while we ask the backend for saved
// devices on mount, "saved" if at least one was found (a chooser listing
// every saved device -- auto-connects to whichever was last used, but any
// other saved device is a tap away, doubling as "switch device" once a
// session is already active), "wizard" for the discover-and-pair flow --
// either because nothing was saved yet, or the user chose to pair another
// device -- and "control" once a control session is open (mute/unmute,
// skip, now playing). Unlike the old single-page version, "control" no
// longer means one specific screen -- see +page.svelte's devicesOpen/
// activeTab for what's actually shown once a session is active.
export type Page = "checking" | "saved" | "wizard" | "control";

export const session = $state({
  page: "checking" as Page,
  savedDevices: [] as SavedDeviceInfo[],
  // The id `checkSaved()` found as last-used -- drives the auto-connect
  // attempt on mount and, once connected, which card the "saved" chooser
  // highlights as current. Distinct from `activeDevice` below: this can
  // point at a device that hasn't actually been (re)connected to yet this
  // launch (e.g. its auto-connect failed and the user hasn't picked
  // anything since).
  lastDeviceId: null as string | null,
  // The device the *current* control session is actually talking to --
  // set from start_control_session's own response, so it's always accurate
  // even after switching devices mid-session. `null` whenever page !==
  // "control".
  activeDevice: null as SavedDeviceInfo | null,
  verifying: false,
  verifyResult: null as "ok" | "failed" | null,
  verifyError: "",
  // Which device verifySaved() is currently acting on / most recently
  // finished acting on, so the chooser can show a per-row spinner and
  // per-row result instead of a page-wide one -- several saved devices sit
  // on screen at once, unlike the old single-card layout. `verifyingId` is
  // only set while the call is in flight; `verifiedId` sticks around after
  // it resolves so the result banner/badge knows which card it belongs to.
  verifyingId: null as string | null,
  verifiedId: null as string | null,

  // True for the duration of one openControls() call -- covers both the
  // auto-connect attempt on launch and a manual device tap. The underlying
  // connect can take several seconds (or, now bounded, time out) against a
  // slow/unreachable Apple TV; without this the chooser looked identical
  // whether nothing had been tried yet or an attempt was quietly still in
  // flight. `connectingId` is which device, for the same per-row reason as
  // `verifyingId`.
  connecting: false,
  connectingId: null as string | null,

  hasLive: false,
  playback: null as PlaybackStatus | null,
  // Local monotonic timestamp (performance.now()) at which `playback` was
  // captured -- lets `livePosition()` below interpolate `playback.position`
  // forward between polls instead of it sitting frozen for up to 250ms
  // (see +page.svelte's poll interval). Reset alongside
  // `playback` every time refreshPlayback() gets a fresh snapshot.
  playbackAnchoredAt: null as number | null,
  // Incremented on a fast local interval (see +page.svelte) purely to give
  // a `$derived.by` that calls `livePosition()` a reason to keep
  // re-evaluating between polls -- the value itself is never read, only
  // written to.
  tick: 0,
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

  // The host the user tapped on the Companion step -- the save step's
  // nickname field defaults to this (most Apple TVs' mDNS host strings are
  // at least somewhat readable, e.g. "Living-Room-Apple-TV"), and it's the
  // fallback name if the field is left untouched. Set in pair(), reset by
  // startPairingWizard().
  pairedHost: "",
  // The save step's editable nickname field.
  deviceName: "",
  // The id finish_pairing() handed back, for the "done" screen's "Open
  // controls" button -- connects straight to the device that was just
  // paired without waiting on a fresh list_saved_devices round trip.
  newDeviceId: null as string | null,
});

// Runs once on mount to decide the starting page: straight into the wizard
// if nothing's ever been saved, or the chooser (session.page = "saved") if
// libs/appletv-cli (or an earlier run of this app) already produced at
// least one saved device. Callers decide what to do next (see
// DevicesPage.svelte, which auto-runs openControls(lastDeviceId) when one
// is set and still among the saved devices).
export async function checkSaved() {
  try {
    const [devices, lastId] = await Promise.all([
      invoke<SavedDeviceInfo[]>("list_saved_devices"),
      invoke<string | null>("last_saved_device_id"),
    ]);
    session.savedDevices = devices;
    session.lastDeviceId = lastId;
    session.page = devices.length > 0 ? "saved" : "wizard";
  } catch (e) {
    // Backend couldn't even check -- fall through to the normal pairing
    // wizard rather than get stuck on "checking...".
    session.error = String(e);
    session.page = "wizard";
  }
}

export async function verifySaved(id: string) {
  session.verifying = true;
  session.verifyingId = id;
  session.verifyResult = null;
  session.verifyError = "";
  try {
    await invoke("verify_saved_pairing", { id });
    session.verifyResult = "ok";
  } catch (e) {
    session.verifyResult = "failed";
    session.verifyError = String(e);
  } finally {
    session.verifying = false;
    session.verifyingId = null;
    session.verifiedId = id;
  }
}

// Renames a saved device in place -- updates the local list optimistically
// on success rather than re-fetching the whole list for a one-field change.
export async function renameDevice(id: string, name: string) {
  try {
    await invoke("rename_saved_device", { id, name });
    const device = session.savedDevices.find((d) => d.id === id);
    if (device) device.name = name;
    if (session.activeDevice?.id === id) session.activeDevice.name = name;
  } catch (e) {
    session.error = String(e);
  }
}

// Removes a saved device. Clears lastDeviceId locally too if it pointed at
// the device just removed, matching what storage::delete_device already
// does backend-side -- otherwise a stale id could linger in memory for the
// rest of this session even though the file (and last_device.store) is
// gone.
export async function deleteDevice(id: string) {
  try {
    await invoke("delete_saved_device", { id });
    session.savedDevices = session.savedDevices.filter((d) => d.id !== id);
    if (session.lastDeviceId === id) session.lastDeviceId = null;
  } catch (e) {
    session.error = String(e);
  }
}

// Runs Pair-Verify + bootstraps a Companion control session for the saved
// device `id` (plus a live MRP/AirPlay session if one was paired for it).
// Available from the chooser, right after a fresh pairing finishes, or
// automatically on launch for the last-used device (see DevicesPage.svelte).
// Replaces whatever control session was already active, so this doubles as
// "switch device" when called again with a different id while page ===
// "control". Returns whether it succeeded so callers can decide what to
// show next -- doesn't touch filter/creation state itself (that's each
// module's own job; +page.svelte composes them after this resolves).
export async function openControls(id: string): Promise<boolean> {
  session.error = "";
  session.connecting = true;
  session.connectingId = id;
  try {
    const info = await invoke<ControlInfo>("start_control_session", { id });
    session.hasLive = info.has_live;
    // Prefer the full record from the last list_saved_devices fetch (it has
    // port/has_mrp/has_airplay, which ControlInfo doesn't carry) -- falls
    // back to a partial one built from ControlInfo alone for a device
    // that's not in that list yet (freshly paired this launch; checkSaved()
    // hasn't re-run since).
    session.activeDevice = session.savedDevices.find((d) => d.id === info.id) ?? {
      id: info.id,
      name: info.name,
      host: info.host,
      port: 0,
      has_mrp: false,
      has_airplay: false,
    };
    session.lastDeviceId = info.id;
    session.playback = null;
    session.controlError = "";
    session.page = "control";
    return true;
  } catch (e) {
    session.error = String(e);
    return false;
  } finally {
    session.connecting = false;
    session.connectingId = null;
  }
}

export async function doSkip(seconds: number) {
  session.controlBusy = true;
  session.controlError = "";
  try {
    await invoke("control_skip", { seconds });
    // Backend actively re-fetches on every control_playback_status call
    // now, but that's on the next poll tick -- fetch immediately too so
    // the display catches up to the skip right away.
    await refreshPlayback();
  } catch (e) {
    session.controlError = String(e);
  } finally {
    session.controlBusy = false;
  }
}

// Absolute jump-to via MRP's SeekToPlaybackPosition (needs the live
// MRP/AirPlay transport, not just Companion -- see control_seek's doc).
// Unlike doSkip, this lands exactly at `position` in one dispatch instead
// of asking Companion to fast-forward/rewind by an amount some apps only
// honor as a fixed, much-shorter hop -- see CueEditorSheet's "jump to test"
// button, the one caller that needs to land at a specific time rather than
// just nudge by a few seconds.
export async function doSeek(position: number) {
  session.controlBusy = true;
  session.controlError = "";
  try {
    await invoke("control_seek", { position });
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
  session.playbackAnchoredAt = performance.now();
}

// Interpolates `playback.position` forward using real elapsed time since it
// was captured, the same anchor-a-value-plus-a-monotonic-instant approach
// the Rust backend already uses for its own extrapolation
// (PlayerSnapshot::position_now) -- so the number on screen keeps advancing
// smoothly instead of sitting frozen for up to a full second between polls.
// Reads `session.tick` purely so a caller's `$derived.by(() => livePosition())`
// picks up the fast local ticker as a dependency; its value is otherwise
// unused. Frozen (not extrapolated) whenever `is_advancing` is false --
// same rule the backend applies, so a paused position doesn't drift ahead
// of what's actually on screen.
export function livePosition(): number | null {
  session.tick;
  const p = session.playback;
  if (p?.position == null) return null;
  if (!p.is_advancing || session.playbackAnchoredAt == null) return p.position;
  const live = p.position + (performance.now() - session.playbackAnchoredAt) / 1000;
  return p.duration != null ? Math.min(live, p.duration) : live;
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
    if (protocol === "companion") {
      // Companion is the required first step, so its host is what the save
      // step's nickname field should default to -- set it here rather than
      // wherever the wizard lands on "save", since that's the one place we
      // definitely still have the tapped device on hand.
      session.pairedHost = device.host;
      session.deviceName = device.host;
    }
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

// Resets the discover-and-pair flow back to its first step and clears out
// whatever the previous run left behind (a stale "done", a leftover device
// list/nickname) -- the entry point for both "nothing saved yet" and "pair
// another device" from the chooser, so either always lands on a clean
// Companion step rather than wherever session.step happened to be left.
export function startPairingWizard() {
  session.page = "wizard";
  session.step = "companion";
  session.devices = [];
  session.error = "";
  session.pairedHost = "";
  session.deviceName = "";
  session.newDeviceId = null;
}

export async function save(name: string) {
  session.pairing = true;
  session.error = "";
  try {
    session.newDeviceId = await invoke<string>("finish_pairing", { name });
    session.step = "done";
  } catch (e) {
    session.error = String(e);
  } finally {
    session.pairing = false;
  }
}
