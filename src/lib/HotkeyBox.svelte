<script lang="ts">
  import { setHotkey } from "./api";
  import { capture, pretty } from "./hotkey";
  import Icon from "./Icon.svelte";
  import type { Action } from "./types";

  interface Props {
    featureId: string;
    action: Action;
  }
  let { featureId, action }: Props = $props();

  let listening = $state(false);
  let hint = $state("");
  let error = $state("");
  let saving = $state(false);

  async function apply(value: string | null) {
    listening = false;
    hint = "";
    error = "";
    saving = true;
    try {
      await setHotkey(featureId, action.id, value);
    } catch (e) {
      error = String(e);
    } finally {
      saving = false;
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (!listening) {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        start();
      }
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    const r = capture(e);
    switch (r.kind) {
      case "pending":
        break;
      case "cancel":
        listening = false;
        hint = "";
        break;
      case "clear":
        apply(null);
        break;
      case "invalid":
        hint = r.message;
        break;
      case "set":
        apply(r.accelerator);
        break;
    }
  }

  function start() {
    error = "";
    hint = "";
    listening = true;
  }
</script>

<div class="hk">
  <div class="line">
    <button
      type="button"
      class="box"
      class:listening
      class:empty={!action.hotkey && !listening}
      disabled={saving}
      aria-label="Hotkey for {action.label}"
      onclick={start}
      {onkeydown}
      onblur={() => {
        listening = false;
        hint = "";
      }}
    >
      <Icon name="keyboard" size={14} />
      <span>
        {#if listening}
          Press keys…
        {:else if action.hotkey}
          {pretty(action.hotkey)}
        {:else}
          Not set
        {/if}
      </span>
    </button>
    <button
      type="button"
      class="icon-btn small"
      title="Clear"
      aria-label="Clear hotkey"
      disabled={!action.hotkey || saving}
      onclick={() => apply(null)}
    >
      <Icon name="x" size={14} />
    </button>
    <button
      type="button"
      class="icon-btn small"
      title={action.defaultHotkey ? `Reset to ${pretty(action.defaultHotkey)}` : "Reset to default (none)"}
      aria-label="Reset hotkey to default"
      disabled={action.hotkey === action.defaultHotkey || saving}
      onclick={() => apply("default")}
    >
      <Icon name="rotate-ccw" size={14} />
    </button>
  </div>
  {#if hint}<div class="hint">{hint}</div>{/if}
  {#if error}
    <div class="hint err" role="alert">{error}</div>
  {/if}
</div>

<style>
  .hk {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 4px;
  }
  .line {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .box {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    min-width: 150px;
    height: 28px;
    padding: 0 10px;
    border-radius: 6px;
    border: 1px solid var(--border);
    border-bottom-color: var(--border-strong);
    background: var(--field);
    font-size: 13px;
    transition:
      border-color 120ms var(--ease),
      background 120ms var(--ease),
      box-shadow 120ms var(--ease);
  }
  .box:hover:not(:disabled) {
    background: var(--control-hover);
  }
  .box.empty {
    color: var(--text-3);
  }
  .box.listening {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
    color: var(--accent-text);
    background: var(--accent-subtle);
  }
  .box:disabled {
    opacity: 0.6;
  }
  .small {
    width: 26px;
    height: 26px;
  }
  .hint {
    font-size: 12px;
    color: var(--text-2);
  }
  .hint.err {
    color: var(--err-text);
    max-width: 280px;
    text-align: right;
    user-select: text;
  }
</style>
