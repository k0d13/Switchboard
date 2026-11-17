<script lang="ts">
  interface Props {
    checked: boolean;
    onchange: (next: boolean) => void;
    disabled?: boolean;
    label: string;
  }
  let { checked, onchange, disabled = false, label }: Props = $props();
</script>

<button
  type="button"
  role="switch"
  aria-checked={checked}
  aria-label={label}
  {disabled}
  class:on={checked}
  onclick={() => onchange(!checked)}
>
  <span class="thumb"></span>
</button>

<style>
  button {
    position: relative;
    flex: none;
    width: 40px;
    height: 20px;
    padding: 0;
    border-radius: 10px;
    border: 1px solid var(--border-strong);
    background: transparent;
    transition:
      background 160ms var(--ease),
      border-color 160ms var(--ease);
  }
  button:hover:not(:disabled) {
    background: var(--sidebar-hover);
  }
  button.on,
  button.on:hover:not(:disabled) {
    background: var(--accent);
    border-color: transparent;
  }
  button:disabled {
    opacity: 0.45;
  }
  .thumb {
    position: absolute;
    top: 50%;
    left: 4px;
    width: 12px;
    height: 12px;
    margin-top: -6px;
    border-radius: 50%;
    background: var(--text-2);
    transition:
      transform 180ms var(--ease),
      width 120ms var(--ease),
      background 160ms var(--ease);
  }
  button:hover:not(:disabled) .thumb {
    width: 14px;
    margin-top: -6px;
  }
  button:active:not(:disabled) .thumb {
    width: 16px;
  }
  button.on .thumb {
    background: var(--on-accent);
    transform: translateX(20px);
  }
  button.on:hover:not(:disabled) .thumb {
    transform: translateX(18px);
  }
  button.on:active:not(:disabled) .thumb {
    transform: translateX(16px);
  }
</style>
