import { getLogs, getState, onLog, onState } from "./api";
import type { AppState, LogLine } from "./types";

const MAX_LOGS = 1000;

/** Latest copy of everything the host has told us. Always re-rendered from the "state" event. */
export const app = $state<{ state: AppState | null; logs: LogLine[] }>({ state: null, logs: [] });

export async function startStore(withLogs: boolean) {
  onState((s) => (app.state = s));
  if (withLogs) {
    onLog((l) => {
      app.logs.push(l);
      if (app.logs.length > MAX_LOGS) app.logs.splice(0, app.logs.length - MAX_LOGS);
    });
  }
  app.state = await getState();
  if (withLogs) {
    const existing = await getLogs();
    // Keep anything that arrived while the request was in flight.
    const newest = existing.length ? existing[existing.length - 1].time : 0;
    app.logs = [...existing, ...app.logs.filter((l) => l.time > newest)];
  }
}
