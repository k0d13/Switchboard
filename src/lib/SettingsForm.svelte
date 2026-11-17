<script lang="ts">
  import { getSchema, setSettings } from "./api";
  import FieldInput from "./FieldInput.svelte";
  import Icon from "./Icon.svelte";
  import type { Field } from "./types";

  interface Props {
    featureId: string;
    settings: Record<string, unknown>;
  }
  let { featureId, settings }: Props = $props();

  let fields = $state<Field[] | null>(null);
  let loadError = $state("");
  let values = $state<Record<string, unknown>>({});
  let saved = $state(false);
  let saveError = $state("");

  // Plain variables on purpose: they must not retrigger the sync effect below.
  let pending = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let savedTimer: ReturnType<typeof setTimeout> | undefined;

  $effect(() => {
    const id = featureId;
    fields = null;
    loadError = "";
    let cancelled = false;
    getSchema(id).then(
      (f) => !cancelled && (fields = f),
      (e) => !cancelled && (loadError = String(e)),
    );
    return () => {
      cancelled = true;
    };
  });

  // Follow the latest state unless the user has edits waiting to be saved.
  $effect(() => {
    const incoming = $state.snapshot(settings);
    if (!pending) values = { ...incoming };
  });

  const sections = $derived.by(() => {
    const out: { title: string | null; fields: Field[] }[] = [];
    for (const f of fields ?? []) {
      const title = f.section ?? null;
      const last = out[out.length - 1];
      if (last && last.title === title) last.fields.push(f);
      else out.push({ title, fields: [f] });
    }
    return out;
  });

  function change(key: string, v: unknown) {
    values[key] = v;
    pending = true;
    saved = false;
    clearTimeout(timer);
    timer = setTimeout(save, 400);
  }

  async function save() {
    clearTimeout(timer);
    pending = false;
    try {
      await setSettings(featureId, $state.snapshot(values));
      saveError = "";
      saved = true;
      clearTimeout(savedTimer);
      savedTimer = setTimeout(() => (saved = false), 2000);
    } catch (e) {
      saveError = String(e);
    }
  }

  $effect(() => () => {
    if (pending) save();
  });
</script>

<div class="settings">
  <div class="title-row">
    <h2>Settings</h2>
    <span class="saved" class:show={saved} aria-live="polite">
      <Icon name="check" size={14} />
      Saved
    </span>
  </div>

  {#if saveError}
    <div class="error-msg" role="alert"><span>{saveError}</span></div>
  {/if}

  {#if loadError}
    <div class="error-msg" role="alert"><span>Couldn't load settings: {loadError}</span></div>
  {:else if fields === null}
    <div class="skeleton" aria-busy="true" aria-label="Loading settings">
      {#each [0, 1, 2, 3] as i (i)}
        <div class="bone" style="animation-delay:{i * 120}ms"></div>
      {/each}
    </div>
  {:else}
    <div class="groups">
      {#each sections as s, si (si)}
        <section>
          {#if s.title}<h3>{s.title}</h3>{/if}
          <div class="card group">
            {#each s.fields as f (f.key)}
              <div class="row" class:stack={f.kind === "list"}>
                <div class="label">
                  <label for={f.kind === "toggle" ? undefined : `f-${featureId}-${f.key}`}>{f.label}</label>
                  {#if f.help}<div class="help">{f.help}</div>{/if}
                </div>
                <div class="control" class:wide={f.kind === "list" || f.kind === "text"}>
                  <FieldInput id={`f-${featureId}-${f.key}`} field={f} value={values[f.key]} onchange={(v) => change(f.key, v)} />
                </div>
              </div>
            {/each}
          </div>
        </section>
      {/each}
    </div>
  {/if}
</div>

<style>
  .settings {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .title-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 24px;
  }
  h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
  }
  .saved {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--tone-ok);
    font-size: 12.5px;
    opacity: 0;
    transform: translateY(2px);
    transition:
      opacity 200ms var(--ease),
      transform 200ms var(--ease);
  }
  .saved.show {
    opacity: 1;
    transform: none;
  }
  .groups {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  h3 {
    margin: 0 0 6px 2px;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-2);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .group {
    padding: 2px 0;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 20px;
    padding: 10px 14px;
  }
  .row + .row {
    border-top: 1px solid var(--border);
  }
  .row.stack {
    flex-direction: column;
    align-items: stretch;
    gap: 8px;
  }
  .label {
    min-width: 0;
    flex: 1;
  }
  label {
    font-weight: 500;
  }
  .help {
    margin-top: 1px;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .control {
    flex: none;
    display: flex;
    justify-content: flex-end;
  }
  .control.wide {
    flex: 0 1 280px;
  }
  .row.stack .control {
    flex: none;
  }
  .skeleton {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .bone {
    height: 52px;
    border-radius: 8px;
    background: var(--card);
    border: 1px solid var(--border);
    animation: pulse 1.2s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.45;
    }
  }
</style>
