const MODIFIER_CODES = new Set([
  "ControlLeft", "ControlRight", "AltLeft", "AltRight", "ShiftLeft", "ShiftRight", "MetaLeft", "MetaRight",
]);

export type Captured =
  | { kind: "pending" }
  | { kind: "cancel" }
  | { kind: "clear" }
  | { kind: "invalid"; message: string }
  | { kind: "set"; accelerator: string };

function keyName(code: string): string | null {
  if (code.startsWith("Key")) return code.slice(3);
  if (code.startsWith("Digit")) return code.slice(5);
  if (/^F([1-9]|1\d|2[0-4])$/.test(code)) return code;
  return code || null;
}

/** Turn a keydown into an accelerator like "Ctrl+Alt+X", or say what to do instead. */
export function capture(e: KeyboardEvent): Captured {
  if (MODIFIER_CODES.has(e.code)) return { kind: "pending" };
  const mods = !e.ctrlKey && !e.altKey && !e.shiftKey && !e.metaKey;
  if (e.code === "Escape" && mods) return { kind: "cancel" };
  if (e.code === "Backspace" && mods) return { kind: "clear" };

  const key = keyName(e.code);
  if (!key) return { kind: "pending" };

  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  if (e.metaKey) parts.push("Super");

  const n = /^F(\d+)$/.exec(key);
  const highFn = n !== null && Number(n[1]) >= 13;
  if (parts.length === 0 && !highFn) {
    return { kind: "invalid", message: "Include Ctrl, Alt, Shift or Win." };
  }
  return { kind: "set", accelerator: [...parts, key].join("+") };
}

/** Display form, e.g. "Super" shown as "Win". */
export function pretty(hotkey: string | null): string {
  return hotkey ? hotkey.replace(/\bSuper\b/g, "Win") : "";
}
