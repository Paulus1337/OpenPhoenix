pub const INDEX_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="dark light">
<title>OpenPhoenix</title>
<link rel="icon" type="image/svg+xml" href="/favicon.svg">
<link rel="stylesheet" href="/style.css">
</head>
<body>
<header>
  <img id="logo" src="/logo.svg" alt="Pip the Phoenix" width="36" height="36">
  <div class="title">
    <h1>OpenPhoenix</h1>
    <div class="tag">rise &amp; shine</div>
  </div>
  <button id="token-btn" type="button" title="Set API token">token</button>
</header>
<main id="log" aria-live="polite"></main>
<form id="composer">
  <textarea id="prompt" rows="3" placeholder="Ask Pip… (Ctrl+Enter to send)" autofocus></textarea>
  <button id="send" type="submit">Send</button>
</form>
<script src="/app.js"></script>
</body>
</html>
"##;

pub const APP_JS: &str = r##""use strict";

const log = document.getElementById("log");
const form = document.getElementById("composer");
const promptEl = document.getElementById("prompt");
const sendBtn = document.getElementById("send");
const tokenBtn = document.getElementById("token-btn");

function token() {
  let t = sessionStorage.getItem("phoenix_token") || "";
  if (!t) {
    t = window.prompt("HTTP API token (http.token / PHOENIX_HTTP_TOKEN):") || "";
    if (t) sessionStorage.setItem("phoenix_token", t.trim());
  }
  return t.trim();
}

tokenBtn.addEventListener("click", () => {
  sessionStorage.removeItem("phoenix_token");
  token();
});

function bubble(cls, text) {
  const div = document.createElement("div");
  div.className = "msg " + cls;
  div.textContent = text;
  log.appendChild(div);
  log.scrollTop = log.scrollHeight;
  return div;
}

const greeting = document.createElement("div");
greeting.className = "greeting";
const pip = document.createElement("img");
pip.src = "/logo.svg";
pip.alt = "Pip the Phoenix";
const hi = document.createElement("div");
hi.className = "hi";
hi.textContent = "Pip here - your phoenix is ready.";
const sub = document.createElement("div");
sub.textContent = "Ask anything below. Ctrl+Enter sends it flying.";
greeting.appendChild(pip);
greeting.appendChild(hi);
greeting.appendChild(sub);
log.appendChild(greeting);

function clearGreeting() {
  if (greeting.parentNode) greeting.remove();
}

async function send() {
  const text = promptEl.value.trim();
  if (!text) return;
  clearGreeting();
  bubble("user", text);
  promptEl.value = "";
  sendBtn.disabled = true;
  const pending = bubble("agent pending", "hatching…");
  try {

    const headers = { "Content-Type": "application/json" };
    const t = sessionStorage.getItem("phoenix_token") || "";
    if (t) headers["Authorization"] = "Bearer " + t;
    const res = await fetch("/run", {
      method: "POST",
      headers: headers,
      body: JSON.stringify({ prompt: text }),
    });
    const data = await res.json().catch(() => ({}));
    if (res.status === 401) {
      sessionStorage.removeItem("phoenix_token");
      const nt = token();
      pending.textContent = nt
        ? "Token set: send your message again."
        : "Unauthorized: no valid credentials.";
      pending.className = "msg err";
    } else if (res.status === 403) {
      pending.textContent =
        "The web chat is switched off on the server. Ask whoever runs it to set " +
        "a username and password for the web UI, then reload this page.";
      pending.className = "msg err";
    } else if (res.status === 429) {
      pending.textContent =
        "Too many failed sign-in attempts from this computer. Wait about five " +
        "minutes, then try again.";
      pending.className = "msg err";
    } else if (res.status === 413 || res.status === 431) {
      pending.textContent = "That message is too long. Try a shorter one.";
      pending.className = "msg err";
    } else if (res.status >= 500) {
      pending.textContent =
        "The server hit a problem answering that. It is still running; try again.";
      pending.className = "msg err";
    } else if (!res.ok) {
      pending.textContent = "Error: " + (data.error || res.status);
      pending.className = "msg err";
    } else {
      pending.textContent = data.reply || "(no visible text)";
      pending.className = "msg agent";
      const files = Array.isArray(data.media) ? data.media : [];
      if (files.length) {
        const note = document.createElement("div");
        note.className = "attached";
        note.textContent =
          files.length === 1
            ? "Attached: " + files[0]
            : "Attached " + files.length + " files: " + files.join(", ");
        pending.appendChild(note);
      }
    }
  } catch (e) {
    pending.textContent =
      "Could not reach the server. Check it is still running, then try again.";
    pending.className = "msg err";
  } finally {
    sendBtn.disabled = false;
    promptEl.focus();
  }
}

form.addEventListener("submit", (e) => { e.preventDefault(); send(); });
promptEl.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); send(); }
});
"##;

pub const STYLE_CSS: &str = r##":root {
  --bg: #14100d;
  --fg: #f2ece4;
  --dim: #a89a8c;
  --accent: #ff8c2e;
  --ember: #f2600f;
  --user: #2b2018;
  --agent: #201914;
  --err: #3a1d1d;
}
* { box-sizing: border-box; }
html, body { height: 100%; }
body {
  margin: 0;
  display: flex;
  flex-direction: column;
  font: 15px/1.5 system-ui, sans-serif;
  color: var(--fg);
  background:
    radial-gradient(60rem 30rem at 50% -12rem, rgba(242, 96, 15, 0.16), transparent 60%),
    var(--bg);
}
header {
  display: flex;
  align-items: center;
  padding: 0.6rem 1rem;
  border-bottom: 1px solid #2b211a;
}
header .title { flex: 1; margin-left: 0.6rem; }
header h1 {
  font-size: 1rem;
  margin: 0;
  background: linear-gradient(90deg, #ffd968, var(--accent), var(--ember));
  -webkit-background-clip: text;
  background-clip: text;
  color: transparent;
}
header .tag { font-size: 0.72rem; color: var(--dim); }
#logo {
  flex: 0 0 auto;
  filter: drop-shadow(0 0 6px rgba(255, 140, 46, 0.55));
  animation: flicker 3.2s ease-in-out infinite;
}
@keyframes flicker {
  0%, 100% { filter: drop-shadow(0 0 5px rgba(255, 140, 46, 0.45)); }
  40%      { filter: drop-shadow(0 0 9px rgba(255, 176, 46, 0.75)); }
  60%      { filter: drop-shadow(0 0 6px rgba(242, 96, 15, 0.6)); }
}
header button {
  background: none;
  border: 1px solid #40342a;
  color: var(--dim);
  border-radius: 6px;
  padding: 0.2rem 0.7rem;
  cursor: pointer;
}
header button:hover { color: var(--accent); border-color: var(--accent); }
#log {
  flex: 1;
  overflow-y: auto;
  padding: 1rem;
  display: flex;
  flex-direction: column;
  gap: 0.6rem;
}
.msg {
  max-width: 46rem;
  padding: 0.55rem 0.8rem;
  border-radius: 12px;
  white-space: pre-wrap;
  word-wrap: break-word;
}
.attached {
  margin-top: 0.45rem;
  padding-top: 0.4rem;
  border-top: 1px solid #322619;
  font-size: 0.78rem;
  color: var(--dim);
}
.msg.user {
  background: var(--user);
  border: 1px solid #3d2c1e;
  align-self: flex-end;
}
.msg.agent {
  background: var(--agent);
  border: 1px solid #322619;
  align-self: flex-start;
  padding-left: 2.4rem;
  position: relative;
}
.msg.agent::before {
  content: "";
  position: absolute;
  left: 0.55rem;
  top: 0.6rem;
  width: 1.35rem;
  height: 1.35rem;
  background: url("/logo.svg") no-repeat center / contain;
}
.msg.err { background: var(--err); align-self: flex-start; color: #f0b9b1; }
.msg.pending { color: var(--dim); }
.msg.pending::before {
  background-image: url("/egg.svg");
  animation: hatch 1.1s ease-in-out infinite;
}
@keyframes hatch {
  0%, 100% { transform: rotate(-7deg); }
  50%      { transform: rotate(7deg) translateY(-1px); }
}
.greeting {
  align-self: center;
  text-align: center;
  color: var(--dim);
  margin-top: 8vh;
}
.greeting img {
  width: 96px;
  height: 96px;
  filter: drop-shadow(0 0 14px rgba(255, 140, 46, 0.5));
  animation: flicker 3.2s ease-in-out infinite;
}
.greeting .hi { color: var(--fg); font-size: 1.05rem; margin-top: 0.6rem; }
#composer {
  display: flex;
  gap: 0.5rem;
  padding: 0.8rem 1rem;
  border-top: 1px solid #2b211a;
}
#prompt {
  flex: 1;
  resize: vertical;
  background: #1c1610;
  color: var(--fg);
  border: 1px solid #40342a;
  border-radius: 8px;
  padding: 0.55rem 0.7rem;
}
#prompt:focus { outline: none; border-color: var(--accent); }
#send {
  align-self: flex-end;
  background: linear-gradient(180deg, var(--accent), var(--ember));
  border: none;
  color: #fff;
  border-radius: 8px;
  padding: 0.55rem 1.2rem;
  cursor: pointer;
}
#send:hover { filter: brightness(1.1); }
#send:disabled { opacity: 0.5; }
"##;
