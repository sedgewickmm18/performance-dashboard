
### Showcase for vibe coding with Chrome-dev MCP


Install with

```
npm i
```

and start with

```
PORT=3000 node server.js
```


Process list shows fake data, all other data is pulled live from the local system.


### Setting up your code agent for chrome-dev MCP

Let your coding agent, for example IBM Bob, set it up for you with the following [skill](https://raw.githubusercontent.com/sedgewickmm18/performance-dashboard/refs/heads/master/chrome-devtools-setup/SKILL.md) `./chrome-dev-MCP/SKILL.md`. The only step remaining is to start chrome in debug mode with

```bash
/opt/google/chrome/chrome \
  --remote-debugging-port=9222 \
  --user-data-dir=/tmp/chrome-debug \
  --no-first-run \
  --disable-extensions \
  "http://localhost:3000"   # replace with your dev server URL
```
