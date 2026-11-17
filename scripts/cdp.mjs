// Drives Switchboard's webviews over the Chrome DevTools Protocol, for testing the real app.
// Start Switchboard with WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333, then:
//   node scripts/cdp.mjs list
//   node scripts/cdp.mjs eval <flyout|settings> "<js expression>"   (or @file.js)
// The expression may be async; its JSON result is printed.

const PORT = process.env.CDP_PORT ?? "9333";
const [cmd, kind, arg] = process.argv.slice(2);
// `@path` reads the expression from a file, which avoids shell quoting trouble
const expr = arg?.startsWith("@") ? (await import("node:fs")).readFileSync(arg.slice(1), "utf8") : arg;

async function targets() {
  const res = await fetch(`http://127.0.0.1:${PORT}/json/list`);
  return (await res.json()).filter((t) => t.type === "page");
}

async function evaluate(target, expression) {
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((ok, fail) => {
    ws.onopen = ok;
    ws.onerror = fail;
  });
  const reply = new Promise((ok) => {
    ws.onmessage = (m) => {
      const msg = JSON.parse(m.data);
      if (msg.id === 1) ok(msg);
    };
  });
  ws.send(
    JSON.stringify({
      id: 1,
      method: "Runtime.evaluate",
      params: { expression, awaitPromise: true, returnByValue: true },
    }),
  );
  const msg = await Promise.race([reply, new Promise((_, no) => setTimeout(() => no(new Error("timed out")), 10000))]);
  ws.close();
  return msg.result?.exceptionDetails ? { error: msg.result.exceptionDetails.exception?.description } : msg.result?.result?.value;
}

const all = await targets();
if (cmd === "list") {
  for (const t of all) {
    const label = await evaluate(t, "window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label ?? location.href");
    console.log(label, t.url);
  }
} else if (cmd === "eval") {
  let found;
  for (const t of all) {
    if ((await evaluate(t, "window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label")) === kind) found = t;
  }
  if (!found) {
    console.log(JSON.stringify({ error: `no ${kind} window` }));
    process.exit(1);
  }
  console.log(JSON.stringify(await evaluate(found, expr), null, 1));
}
