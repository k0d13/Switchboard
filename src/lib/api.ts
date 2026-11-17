// Typed wrappers around the Tauri commands and events in types.ts. Outside Tauri (plain browser
// via `npm run dev`) everything is served by an in-memory mock so the UI can be previewed.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { AppState, FeatureState, Field, LogLine } from "./types";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export type WindowKind = "flyout" | "settings";

export function windowKind(): WindowKind {
  if (inTauri) return getCurrentWindow().label === "flyout" ? "flyout" : "settings";
  return location.hash.replace("#", "") === "flyout" ? "flyout" : "settings";
}

type Unlisten = () => void;

// ---------------------------------------------------------------- public API

export function getState(): Promise<AppState> {
  return inTauri ? invoke<AppState>("get_state") : mock.getState();
}
export function getSchema(id: string): Promise<Field[]> {
  return inTauri ? invoke<Field[]>("get_schema", { id }) : mock.getSchema(id);
}
export function setEnabled(id: string, enabled: boolean): Promise<void> {
  return inTauri ? invoke("set_enabled", { id, enabled }) : mock.setEnabled(id, enabled);
}
export function setSettings(id: string, settings: Record<string, unknown>): Promise<void> {
  return inTauri ? invoke("set_settings", { id, settings }) : mock.setSettings(id, settings);
}
export function runAction(id: string, action: string): Promise<void> {
  return inTauri ? invoke("run_action", { id, action }) : mock.runAction(id, action);
}
export function setHotkey(id: string, action: string, hotkey: string | null): Promise<void> {
  return inTauri ? invoke("set_hotkey", { id, action, hotkey }) : mock.setHotkey(id, action, hotkey);
}
export function setAutostart(enabled: boolean): Promise<void> {
  return inTauri ? invoke("set_autostart", { enabled }) : mock.setAutostart(enabled);
}
export function getLogs(): Promise<LogLine[]> {
  return inTauri ? invoke<LogLine[]>("get_logs") : mock.getLogs();
}
export function openSettings(feature?: string): Promise<void> {
  return inTauri ? invoke("open_settings", { feature }) : mock.openSettings(feature);
}
export function flyoutResize(height: number): Promise<void> {
  return inTauri ? invoke("flyout_resize", { height }) : Promise.resolve();
}
export function openUrl(url: string): Promise<void> {
  if (inTauri) return invoke("open_url", { url });
  window.open(url, "_blank", "noopener");
  return Promise.resolve();
}
export function quit(): Promise<void> {
  return inTauri ? invoke("quit") : Promise.resolve();
}

function subscribe<T>(event: string, cb: (payload: T) => void): Unlisten {
  if (!inTauri) return mock.on(event, cb as (p: unknown) => void);
  let dead = false;
  let un: Unlisten | null = null;
  listen<T>(event, (e) => cb(e.payload)).then((u) => {
    if (dead) u();
    else un = u;
  });
  return () => {
    dead = true;
    un?.();
  };
}
export const onState = (cb: (s: AppState) => void) => subscribe<AppState>("state", cb);
export const onLog = (cb: (l: LogLine) => void) => subscribe<LogLine>("log", cb);
export const onNavigate = (cb: (id: string) => void) => subscribe<string>("navigate", cb);

// Window helpers (no-ops in the browser).
export const minimiseWindow = () => (inTauri ? getCurrentWindow().minimize() : Promise.resolve());
export const closeWindow = () => (inTauri ? getCurrentWindow().close() : Promise.resolve());

// ---------------------------------------------------------------- mock

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

function createMock() {
  const listeners = new Map<string, Set<(p: unknown) => void>>();
  const emit = (event: string, payload: unknown) => {
    for (const cb of listeners.get(event) ?? []) cb(structuredClone(payload));
  };

  let signedIn = true;
  let paused = false;
  let xbox = false;
  let signInTimer: ReturnType<typeof setTimeout> | undefined;

  const now = Date.now();
  const logs: LogLine[] = [
    { time: now - 94000, level: "info", source: "app", message: "Switchboard 0.1.0 started" },
    { time: now - 91000, level: "info", source: "xbox", message: "Signed in to Xbox as Gamertag" },
    { time: now - 76000, level: "info", source: "xbox", message: "Turning the Xbox on" },
    { time: now - 70000, level: "info", source: "xbox", message: "XBOX is on" },
    { time: now - 69000, level: "info", source: "xbox", message: "Xbox mode: Acme 27Q on HDMI 2" },
    { time: now - 61000, level: "info", source: "xbox-presence", message: "Connected to Discord as Gamertag" },
    { time: now - 40000, level: "info", source: "xbox-presence", message: "Now showing: Forza Horizon 6" },
    { time: now - 22000, level: "warn", source: "xbox-presence", message: "Discord disconnected, waiting for it to come back" },
  ];

  const xboxFeature: FeatureState = {
    id: "xbox",
    name: "Xbox mode",
    description: "Put your Xbox on a monitor while it's on, and give the monitor back when it's off",
    icon: "gamepad",
    enabled: true,
    status: null,
    busy: false,
    prompt: null,
    actions: [],
    settings: {
      monitorModel: "Acme 27Q", xboxInput: 18, pcInput: 15, xboxLiveId: "FD00112233445566", offInPcMode: false,
      switchDelayMs: 500, wakeTimeoutSecs: 10, xboxBootSecs: 30, xboxAddress: "", xboxName: "XBOX",
      msClientId: "",
    },
  };
  const presenceFeature: FeatureState = {
    id: "xbox-presence",
    name: "Xbox presence",
    description: "Show what you're playing on Xbox in your Discord status",
    icon: "message-circle",
    enabled: true,
    status: null,
    busy: false,
    prompt: null,
    actions: [],
    settings: {
      discordClientId: "", ignoredTitles: ["750323071", "1022622766", "714681658"], msClientId: "",
    },
  };
  const features: FeatureState[] = [xboxFeature, presenceFeature];

  const schemas: Record<string, Field[]> = {
    "xbox-presence": [
      { key: "ignoredTitles", label: "Ignored title IDs", kind: "list", section: "Advanced", help: "Dashboard apps that shouldn't count as playing" },
      { key: "discordClientId", label: "Discord application ID", kind: "text", section: "Advanced", placeholder: "Discord's Xbox app" },
      {
        key: "msClientId", label: "Microsoft client ID", kind: "text", section: "Advanced", placeholder: "Xbox app's public client",
        help: "Your own Azure app registration's client ID. Needs its own sign-in.",
      },
    ],
    xbox: [
      {
        key: "xboxLiveId", label: "Console", kind: "select", section: "Xbox",
        help: "Only an Xbox that's on shows up here. To turn it on and off, sign in above, turn on Remote features in the Xbox's settings, and set its power mode to Sleep.",
        options: [
          { value: "", label: "None" },
          { value: "FD00112233445566", label: "XBOX (192.168.1.20)" },
        ],
      },
      { key: "offInPcMode", label: "Turn the Xbox off when switching to PC mode", kind: "toggle", section: "Xbox" },
      {
        key: "monitorModel", label: "Monitor", kind: "select", section: "Monitor",
        help: "The one the Xbox is plugged into. It needs DDC/CI turned on in its own menu.",
        options: [
          { value: "Acme 27Q", label: "Acme 27Q" },
          { value: "Acme 32U", label: "Acme 32U" },
        ],
      },
      { key: "xboxInput", label: "Xbox input", kind: "select", section: "Monitor", options: inputs() },
      { key: "pcInput", label: "PC input", kind: "select", section: "Monitor", options: inputs(), help: "Updated to whichever input the PC was on each time Xbox mode starts" },
      { key: "switchDelayMs", label: "Wait before moving windows (ms)", kind: "number", min: 0, max: 5000, section: "Advanced" },
      { key: "wakeTimeoutSecs", label: "Wait for the monitor to wake (seconds)", kind: "number", min: 1, max: 60, section: "Advanced" },
      { key: "xboxBootSecs", label: "Wait for the Xbox to turn on (seconds)", kind: "number", min: 5, max: 120, section: "Advanced" },
      { key: "xboxAddress", label: "Xbox IP address", kind: "text", section: "Advanced", placeholder: "Found automatically" },
    ],
  };

  function inputs() {
    return [
      { value: 15, label: "DisplayPort 1" },
      { value: 16, label: "DisplayPort 2" },
      { value: 17, label: "HDMI 1" },
      { value: 18, label: "HDMI 2" },
      { value: 27, label: "USB-C" },
    ];
  }

  function log(level: LogLine["level"], source: string, message: string) {
    const line: LogLine = { time: Date.now(), level, source, message };
    logs.push(line);
    emit("log", line);
  }

  function refresh() {
    xboxFeature.status = { text: xbox ? "Xbox mode" : "PC mode", tone: xbox ? "active" : "ok" };
    presenceFeature.status = !signedIn
      ? { text: "Not signed in", tone: "warn" }
      : paused
        ? { text: "Presence paused", tone: "idle" }
        : xbox
          ? { text: "Playing Forza Horizon 6", tone: "active" }
          : null;
    const hotkey = (f: FeatureState, id: string) => f.actions.find((a) => a.id === id)?.hotkey;
    const signAction = signedIn
      ? { id: "sign-out", label: "Sign out", icon: "log-out", defaultHotkey: null, hotkey: null, hotkeyable: false, active: false }
      : { id: "sign-in", label: "Sign in", icon: "log-in", defaultHotkey: null, hotkey: null, hotkeyable: false, active: false };
    xboxFeature.actions = [
      { id: "xbox-mode", label: "Xbox mode", icon: "gamepad", defaultHotkey: "Ctrl+Alt+X", hotkey: hotkey(xboxFeature, "xbox-mode") ?? "Ctrl+Alt+X", hotkeyable: true, active: xbox },
      { id: "pc-mode", label: "PC mode", icon: "monitor", defaultHotkey: "Ctrl+Alt+P", hotkey: hotkey(xboxFeature, "pc-mode") ?? "Ctrl+Alt+P", hotkeyable: true, active: !xbox },
      { ...signAction },
    ];
    presenceFeature.actions = signedIn
      ? [
          { id: "pause", label: paused ? "Resume presence" : "Pause presence", icon: paused ? "play" : "pause", defaultHotkey: null, hotkey: hotkey(presenceFeature, "pause") ?? null, hotkeyable: true, active: paused },
          { ...signAction },
        ]
      : [{ ...signAction }];
  }

  function snapshot(): AppState {
    return { version: "0.1.0", autostart, features };
  }
  let autostart = true;
  const push = () => emit("state", snapshot());
  refresh();

  const find = (id: string) => {
    const f = features.find((x) => x.id === id);
    if (!f) throw `Unknown feature ${id}`;
    return f;
  };

  return {
    on(event: string, cb: (p: unknown) => void): Unlisten {
      let set = listeners.get(event);
      if (!set) listeners.set(event, (set = new Set()));
      set.add(cb);
      return () => set.delete(cb);
    },
    async getState() {
      await sleep(60);
      return structuredClone(snapshot());
    },
    async getSchema(id: string) {
      await sleep(1200);
      return structuredClone(schemas[id] ?? []);
    },
    async setEnabled(id: string, enabled: boolean) {
      find(id).enabled = enabled;
      log("info", id, enabled ? "Enabled" : "Disabled");
      push();
    },
    async setSettings(id: string, settings: Record<string, unknown>) {
      await sleep(150);
      find(id).settings = structuredClone(settings);
      push();
    },
    async runAction(id: string, action: string) {
      const f = find(id);
      f.busy = true;
      push();
      await sleep(600);
      f.busy = false;
      if (action === "xbox-mode" || action === "pc-mode") {
        xbox = action === "xbox-mode";
        log("info", id, xbox ? "Switched to Xbox mode (HDMI 2)" : "Switched to PC mode (DisplayPort 1)");
      } else if (action === "pause") {
        paused = !paused;
        log("info", id, paused ? "Presence paused" : "Presence resumed");
      } else if (action === "sign-out") {
        signedIn = false;
        paused = false;
        log("info", id, "Signed out");
      } else if (action === "sign-in") {
        f.prompt = {
          title: "Sign in to Xbox",
          message: "Open microsoft.com/link and enter this code.",
          code: "F7KQ2MZP",
          url: "https://www.microsoft.com/link",
        };
        log("info", id, "Waiting for sign-in code to be entered");
        clearTimeout(signInTimer);
        signInTimer = setTimeout(() => {
          f.prompt = null;
          signedIn = true;
          refresh();
          log("info", id, "Signed in as Gamertag");
          push();
        }, 12000);
      }
      refresh();
      push();
    },
    async setHotkey(id: string, action: string, hotkey: string | null) {
      await sleep(80);
      const a = find(id).actions.find((x) => x.id === action);
      if (!a) throw `Unknown action ${action}`;
      const next = hotkey === "default" ? a.defaultHotkey : hotkey;
      if (next === "Ctrl+Alt+Q") throw "Ctrl+Alt+Q is already used by another app.";
      a.hotkey = next;
      push();
    },
    async setAutostart(enabled: boolean) {
      autostart = enabled;
      push();
    },
    async getLogs() {
      return structuredClone(logs);
    },
    async openSettings(feature?: string) {
      if (feature) emit("navigate", feature);
    },
  };
}

const mock = createMock();
