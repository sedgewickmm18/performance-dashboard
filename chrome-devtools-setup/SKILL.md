---
name: chrome-devtools-setup
description: Use when the user wants to set up, configure, or troubleshoot the chrome-devtools MCP server to connect Bob to a running Chrome browser instance for live DOM inspection and browser automation.
---

# Chrome DevTools MCP Setup

Follow these steps to connect Bob's `chrome-devtools` MCP tools to a running Chrome browser.

## Overview

The `chrome-devtools-mcp` package launches its own headless browser by default. To connect it to
an **existing** Chrome window (e.g. to inspect a running dev server), you must:

1. Launch Chrome with `--remote-debugging-port` and a dedicated `--user-data-dir`
2. Point the MCP server at that port via `--browserUrl`

---

## Step 1 — Launch Chrome with remote debugging enabled

Run this in a terminal (keep it open; Chrome must stay running):

```bash
/opt/google/chrome/chrome \
  --remote-debugging-port=9222 \
  --user-data-dir=/tmp/chrome-debug \
  --no-first-run \
  --disable-extensions \
  "http://localhost:3456"   # replace with your dev server URL
```

**Why `--user-data-dir` is required:** Chrome silently ignores `--remote-debugging-port` when it
reuses an existing profile directory. A fresh temporary directory guarantees the port is opened.

Verify the port is active:

```bash
curl -s http://localhost:9222/json | grep '"title"'
```

You should see your page title in the output.

---

## Step 2 — Configure the chrome-devtools MCP server

Edit `~/.bob/settings/mcp.json` (global) or `.bob/mcp.json` (workspace) and add:

```json
{
  "mcpServers": {
    "chrome-devtools": {
      "command": "npx",
      "args": ["-y", "chrome-devtools-mcp@latest", "--browserUrl", "http://127.0.0.1:9222"]
    }
  }
}
```

**Common mistake:** passing `browser-url=http://...` as a positional argument does **not** work —
the server ignores unknown positional args. The flag must be `--browserUrl` (camelCase).

Other useful flags:

| Flag | Purpose |
|---|---|
| `--browserUrl <url>` | Connect to existing Chrome via HTTP endpoint |
| `--wsEndpoint <ws-url>` | Connect via raw WebSocket URL instead |
| `--headless` | Launch a new headless Chrome (no existing instance needed) |
| `--viewport 1280x900` | Set viewport when launching a new instance |

---

## Step 3 — Reload the MCP server in Bob

After editing `mcp.json`, reload the server:

1. Open Bob's **Settings** panel
2. Go to the **MCP** tab
3. Find `chrome-devtools` and click **Reload**

Then verify with:

```javascript
// Bob tool call — should list your page
mcp__chrome-devtools__list_pages()
```

---

## Step 4 — Use the tools

Once connected, core tools available:

| Tool | Purpose |
|---|---|
| `list_pages` | List open browser tabs |
| `take_screenshot` | Capture viewport as image |
| `take_snapshot` | Capture a11y tree with element `uid`s for interaction |
| `click` / `fill` / `drag` | Interact with page elements by `uid` |
| `evaluate_script` | Run arbitrary JavaScript in the page |
| `list_console_messages` | Read the browser console log |
| `list_network_requests` | Inspect XHR/fetch traffic |

**Accessing React internals via `evaluate_script`:**

```javascript
() => {
  const el = document.querySelector('[aria-label="My Component"]');
  const fiberKey = Object.keys(el).find(k => k.startsWith('__reactFiber'));
  let node = el[fiberKey];
  while (node) {
    if (node.memoizedProps?.someController) {
      return node.memoizedProps.someController.getData();
    }
    node = node.return;
  }
  return 'not found';
}
```

---

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `Missing X server` error | MCP tool is trying to spawn its own browser, not connect | Ensure `--browserUrl` flag is in `args`, reload MCP server |
| `curl` to port 9222 returns nothing | Chrome launched without `--user-data-dir` | Kill Chrome, relaunch with explicit `--user-data-dir` |
| `list_pages` shows no page | Chrome is running but page hasn't loaded yet | Wait for page load, retry |
| `evaluate_script` returns `not found` | React fiber traversal depth too shallow | Increase depth limit (try 100 instead of 50) |
