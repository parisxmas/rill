// The account, and the valleys a signed-in person can make.
//
// Everything here talks to `/api/*` on the page's own origin — the game
// server answers those and keeps the records in the store all the valleys
// share, so an account made through one door works at all of them.
//
// The word somebody types is never sent. What travels is a key derived from
// it in the browser: PBKDF2-HMAC-SHA-256, six hundred thousand rounds, which
// is this decade's ordinary cost for turning a guessable word into work. The
// server salts and stretches what arrives once more before storing it — see
// passwd.rill for the division of labour and what it does and does not buy.
// What the split fixes is where the expensive half runs: on the machine with
// one person waiting, not on the one that owes sixteen people a snapshot
// every fifty milliseconds.

const ROUNDS = 600000;

// Deterministic, per-name. The salt's job on this side is only to make the
// key different for the same word under different names — the unpredictable
// salt lives on the server, where the record is.
async function derivedKey(who, word) {
  const enc = new TextEncoder();
  const raw = await crypto.subtle.importKey("raw", enc.encode(word), "PBKDF2", false, ["deriveBits"]);
  const bits = await crypto.subtle.deriveBits(
    { name: "PBKDF2", hash: "SHA-256", salt: enc.encode("rill-valley:" + who), iterations: ROUNDS },
    raw, 256);
  return [...new Uint8Array(bits)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

// One POST, line-oriented body, JSON back. The body is `key=value` lines
// because that is what the server's little reader speaks — see door.rill for
// why it is not JSON in that direction.
async function ask(path, fields) {
  const body = Object.entries(fields).map(([k, v]) => `${k}=${v}`).join("\n") + "\n";
  const r = await fetch(new URL(path, location.origin), { method: "POST", body, cache: "no-store" });
  return r.json();
}

export function accountState() {
  return {
    who: localStorage.getItem("valley.acct.who") ?? "",
    token: localStorage.getItem("valley.acct.token") ?? "",
  };
}

function keep(who, token) {
  localStorage.setItem("valley.acct.who", who);
  localStorage.setItem("valley.acct.token", token);
}

export function forget() {
  localStorage.removeItem("valley.acct.who");
  localStorage.removeItem("valley.acct.token");
}

export async function register(who, word) {
  return ask("/api/register", { who, key: await derivedKey(who, word) })
    .then((r) => { if (r.ok) keep(r.who, r.token); return r; });
}

export async function signIn(who, word) {
  return ask("/api/login", { who, key: await derivedKey(who, word) })
    .then((r) => { if (r.ok) keep(r.who, r.token); return r; });
}

export async function makeValley(fields) {
  return ask("/api/make", { token: accountState().token, ...fields });
}

export async function shutValley(name) {
  return ask("/api/shut", { token: accountState().token, name });
}

export async function myValleys() {
  const r = await ask("/api/mine", { token: accountState().token });
  return r.ok ? r.valleys : [];
}

// ---- the panel on the entry card -------------------------------------------
//
// Three states: signed out (two fields, two buttons), signed in (a make-a-
// valley form and the list of yours), and busy (the derivation takes a
// moment — six hundred thousand rounds is meant to be felt by a machine,
// and on a phone it is felt by the person too, so the button says why).

export function mountAccount(onValleysChanged) {
  const panel = document.getElementById("acct");
  if (!panel) return;
  panel.style.display = "block";

  const paint = () => {
    const { who, token } = accountState();
    if (!token) { paintOut(); return; }
    paintIn(who);
  };

  const paintOut = () => {
    panel.innerHTML = `
      <div class="acctrow">
        <input id="a-who" maxlength="12" placeholder="ACCOUNT" autocomplete="username">
        <input id="a-word" type="password" maxlength="64" placeholder="WORD" autocomplete="current-password">
        <button id="a-in">sign in</button>
        <button id="a-new">register</button>
      </div>
      <small id="a-note"></small>`;
    const note = panel.querySelector("#a-note");
    const go = async (fn) => {
      const who = panel.querySelector("#a-who").value.trim().toLowerCase();
      const word = panel.querySelector("#a-word").value;
      if (!who || !word) { note.textContent = "a name and a word, both"; return; }
      note.textContent = "working the word...";
      try {
        const r = await fn(who, word);
        if (!r.ok) { note.textContent = r.why; return; }
        paint();
        onValleysChanged?.();
      } catch (e) { note.textContent = "the server is not answering"; }
    };
    panel.querySelector("#a-in").addEventListener("click", (e) => { e.preventDefault(); go(signIn); });
    panel.querySelector("#a-new").addEventListener("click", (e) => { e.preventDefault(); go(register); });
  };

  const paintIn = async (who) => {
    panel.innerHTML = `
      <div class="acctrow"><span class="me">${who}</span>
        <button id="a-make">make a valley</button>
        <button id="a-out">sign out</button></div>
      <div id="a-mine"></div>
      <form id="a-form" style="display:none">
        <div class="acctrow">
          <input id="v-name" maxlength="12" placeholder="VALLEY NAME">
          <label>players <input id="v-players" type="number" min="2" max="32" value="16"></label>
          <label>time <select id="v-time">
            <option>noon</option><option>dawn</option><option>dusk</option><option>night</option>
          </select></label>
        </div>
        <div class="acctrow">
          <label>gravity <input id="v-gravity" type="number" min="40" max="900" value="260"></label>
          <label>pines <input id="v-pines" type="number" min="0" max="1200" value="400"></label>
          <label>crystals <input id="v-crystals" type="number" min="0" max="800" value="300"></label>
        </div>
        <div class="acctrow">
          <label>grass <input id="v-grass" type="number" min="0" max="3000" value="900"></label>
          <label>match <input id="v-match" type="number" min="60" max="7200" value="1800"></label>
          <label>meteors <input id="v-meteors" type="checkbox"></label>
          <button id="v-go">make it</button>
        </div>
        <small id="a-note"></small>
      </form>`;
    panel.querySelector("#a-out").addEventListener("click", (e) => {
      e.preventDefault(); forget(); paint();
    });
    const form = panel.querySelector("#a-form");
    panel.querySelector("#a-make").addEventListener("click", (e) => {
      e.preventDefault();
      form.style.display = form.style.display === "none" ? "block" : "none";
    });
    form.addEventListener("submit", (e) => e.preventDefault());
    panel.querySelector("#v-go").addEventListener("click", async (e) => {
      e.preventDefault();
      const note = panel.querySelector("#a-note");
      const val = (id) => panel.querySelector(id).value;
      const r = await makeValley({
        name: val("#v-name").trim().toLowerCase(),
        players: val("#v-players"), time: val("#v-time"),
        gravity: val("#v-gravity"), pines: val("#v-pines"),
        crystals: val("#v-crystals"), grass: val("#v-grass"),
        match: val("#v-match"),
        meteors: panel.querySelector("#v-meteors").checked ? 1 : 0,
      });
      if (!r.ok) { note.textContent = r.why; return; }
      // The keeper starts it within its ten-second walk; the list of doors
      // will show it once it has written itself down. Say so rather than
      // pretending it is instant.
      note.textContent = `${r.name} is being raised at ${r.path} — it appears among the doors in a few seconds`;
      showMine();
      onValleysChanged?.(r.path);
    });
    showMine();
  };

  const showMine = async () => {
    const box = panel.querySelector("#a-mine");
    if (!box) return;
    const mine = await myValleys();
    box.innerHTML = mine.map((v) => `
      <div class="acctrow minerow" data-name="${v.name}">
        <span>${v.name}</span><i>${v.players} players · ${v.time} · ${v.meteors ? "meteors" : "calm"}</i>
        <button data-shut="${v.name}">shut</button>
      </div>`).join("");
    for (const b of box.querySelectorAll("[data-shut]")) {
      b.addEventListener("click", async (e) => {
        e.preventDefault();
        await shutValley(b.dataset.shut);
        showMine();
        onValleysChanged?.();
      });
    }
  };

  paint();
}
