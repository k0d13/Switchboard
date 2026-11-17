// The contract between the Rust host (src-tauri) and the UI. Rust serialises these with serde
// (camelCase), so keep both sides in step when changing anything here.
//
// Commands (invoke):
//   get_state(): AppState
//   get_schema({ id }): Field[]                    slow-ish (may query hardware), call when a feature page opens
//   set_enabled({ id, enabled }): void
//   set_settings({ id, settings }): void          whole settings object for that feature
//   run_action({ id, action }): void              rejects with a message string on failure
//   set_hotkey({ id, action, hotkey }): void      hotkey: accelerator string, null = unbound, "default" = reset
//                                                  rejects with a message if invalid or already taken
//   set_autostart({ enabled }): void
//   get_logs(): LogLine[]
//   open_settings({ feature? }): void             opens (or focuses) the settings window, optionally on a feature
//   flyout_resize({ height }): void               flyout content height in CSS px; host keeps it anchored above the tray
//   open_url({ url }): void                       opens in the default browser (https only)
//   quit(): void
//
// Events (listen):
//   "state"  payload AppState      whenever anything changes (status, busy, prompt, settings, enabled…)
//   "log"    payload LogLine       each new log line
//   "navigate" payload string      settings window should show this feature id (sent by open_settings)
//
// Windows: label "flyout" (tray left click, small, borderless, hides on blur) and label "settings".

export type Tone = "idle" | "ok" | "active" | "warn" | "error";

export interface Status {
  text: string;
  tone: Tone;
}

/** Something the user needs to do, e.g. enter a sign-in code. Shown as a card on the feature. */
export interface Prompt {
  title: string;
  message: string;
  code?: string | null;
  url?: string | null;
}

export interface Action {
  id: string;
  label: string;
  /** Icon name from the UI's icon set (see Icon.svelte); unknown names fall back to a dot. */
  icon?: string | null;
  /** Current binding, null when unbound. */
  hotkey: string | null;
  defaultHotkey: string | null;
  /** Whether this action can have a hotkey at all. */
  hotkeyable: boolean;
  /** Highlight as the current state, e.g. "Xbox mode" while in Xbox mode. */
  active: boolean;
}

export interface FeatureState {
  id: string;
  name: string;
  description: string;
  icon: string;
  enabled: boolean;
  status: Status | null;
  /** An action is running. */
  busy: boolean;
  prompt: Prompt | null;
  actions: Action[];
  settings: Record<string, unknown>;
}

export interface AppState {
  version: string;
  autostart: boolean;
  features: FeatureState[];
}

export type FieldKind = "text" | "number" | "toggle" | "select" | "hex" | "list";

export interface FieldOption {
  value: string | number;
  label: string;
}

/** One setting. The settings page is generated from these. */
export interface Field {
  key: string;
  label: string;
  help?: string | null;
  kind: FieldKind;
  /** For "select". */
  options?: FieldOption[] | null;
  /** For "number" and "hex". */
  min?: number | null;
  max?: number | null;
  placeholder?: string | null;
  /** Group heading; consecutive fields with the same section are grouped. */
  section?: string | null;
}
// "hex": a number shown and edited as 0x.. (e.g. 0x12)
// "list": an array of strings, edited as one item per line

export type Level = "info" | "warn" | "error";

export interface LogLine {
  /** Unix ms */
  time: number;
  level: Level;
  /** Feature id, or "app" */
  source: string;
  message: string;
}
