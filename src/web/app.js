"use strict";

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
    } else if (!res.ok) {
      pending.textContent = "Error: " + (data.error || res.status);
      pending.className = "msg err";
    } else {
      pending.textContent = data.reply || "(empty reply)";
      pending.className = "msg agent";
    }
  } catch (e) {
    pending.textContent = "Request failed: " + e;
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
