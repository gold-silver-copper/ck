// Run in the page ck-web is on: a page of the site posted to (its robots.txt, where there's
// one: no ads, no scripts of its own), so what's sent from here carries the site's cookies.
// What it finds goes to ck-web on the console, as "ck-web:" and JSON.
(() => {
  if (window.ckweb) return;
  const say = (m) => console.log("ck-web:" + JSON.stringify(m));
  // A check before the site (Cloudflare's, DDoS-Guard's) is for a person: shown in the
  // browser view. Once it's passed, the page loads again and this runs again.
  if (/just a moment|attention required|checking your browser|ddos-guard|ddos protection|hold on/i.test(document.title)) {
    say({ is: "human" });
    return;
  }
  const SYS = "https://sys.4chan.org";
  // Where 4chan's captcha keeps its ticket (a wait already served), as its own script does.
  const TICKET = "4chan-tc-ticket";
  let cover = null;
  let wanted = null;

  const clear = () => {
    if (cover) cover.remove();
    cover = null;
  };

  // A white sheet over the page: what the browser view shows.
  const sheet = () => {
    clear();
    cover = document.createElement("div");
    cover.style = "position:fixed;inset:0;margin:0;background:#fff;z-index:2147483647";
    document.documentElement.appendChild(cover);
    return cover;
  };

  // 4chan's captcha comes from a frame on sys.4chan.org, which passes it up with postMessage.
  const load = (ticketResp) => {
    const q = ["ext=1", "board=" + encodeURIComponent(wanted.board)];
    if (wanted.thread > 0) q.push("thread_id=" + wanted.thread);
    if (ticketResp) q.push("ticket_resp=" + encodeURIComponent(ticketResp));
    const ticket = localStorage.getItem(TICKET);
    if (ticket) q.push("ticket=" + encodeURIComponent(ticket));
    const frame = document.createElement("iframe");
    frame.src = SYS + "/captcha?" + q.join("&");
    frame.style = "border:0;width:100%;height:100%;display:block";
    sheet().appendChild(frame);
    say({ is: "loading" });
  };

  // A captcha widget of a service's (hCaptcha, reCAPTCHA, Turnstile, Yandex's), on a sheet in
  // the browser view for a person to do; `done` gets its token.
  const WIDGETS = {
    hcaptcha: ["https://js.hcaptcha.com/1/api.js?render=explicit&recaptchacompat=off&onload=", () => window.hcaptcha],
    recaptcha: ["https://www.google.com/recaptcha/api.js?render=explicit&onload=", () => window.grecaptcha],
    turnstile: ["https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit&onload=", () => window.turnstile],
    yandex: ["https://smartcaptcha.yandexcloud.net/captcha.js?render=onload&onload=", () => window.smartCaptcha],
  };
  const widget = (provider, sitekey, done) => {
    const kind = WIDGETS[provider];
    if (!kind) return say({ is: "failed", error: "Unknown captcha: " + provider });
    const box = document.createElement("div");
    box.style = "display:flex;justify-content:center;padding-top:16px";
    sheet().appendChild(box);
    window.ckwebWidget = () => kind[1]().render(box, { sitekey, callback: done });
    if (kind[1]()) {
      window.ckwebWidget();
    } else {
      const s = document.createElement("script");
      s.src = kind[0] + "ckwebWidget";
      s.onerror = () => say({ is: "failed", error: "Couldn't load the " + provider + " captcha" });
      (document.head || document.documentElement).appendChild(s);
    }
    say({ is: "human" });
  };

  window.addEventListener("message", (e) => {
    if (e.origin !== SYS || !e.data || !e.data.twister) return;
    const t = e.data.twister;
    if (t.ticket) localStorage.setItem(TICKET, t.ticket);
    else if (t.ticket === false) localStorage.removeItem(TICKET);
    // 4chan asks for an hCaptcha first (to earn a ticket) when it's been busy: then the
    // captcha is asked for again with its answer.
    if (t.mpcd) return widget("hcaptcha", t.sitekey, (resp) => load(resp));
    clear();
    say({ is: "captcha", twister: t });
  });

  const base64 = (buf) => {
    let s = "";
    const bytes = new Uint8Array(buf);
    for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    return btoa(s);
  };

  // A request from this page, with its cookies: the fields as a form, if there are any.
  const send = async (method, url, headers, fields, file, raw) => {
    let body = raw === null ? undefined : raw;
    if (body === undefined && (fields.length || file)) {
      body = new FormData();
      for (const [k, v] of fields) body.append(k, v);
      if (file) {
        const bytes = Uint8Array.from(atob(file.data), (c) => c.charCodeAt(0));
        body.append(file.field, new Blob([bytes]), file.name);
      }
    }
    const head = Object.fromEntries(headers.filter(([k]) => k.toLowerCase() !== "referer"));
    const referrer = (headers.find(([k]) => k.toLowerCase() === "referer") || [])[1];
    let r;
    try {
      r = await fetch(url, { method, body, headers: head, referrer, credentials: "include" });
    } catch (e) {
      return say({ is: "failed", error: "Couldn't reach " + new URL(url, location.href).host + ": " + e });
    }
    const image = (r.headers.get("Content-Type") || "").startsWith("image/");
    const content = image ? base64(await r.arrayBuffer()) : await r.text();
    say({ is: "fetched", status: r.status, body: content, cookies: document.cookie });
  };

  window.ckweb = {
    captcha: (board, thread) => {
      wanted = { board, thread };
      load(null);
    },
    widget: (provider, sitekey) =>
      widget(provider, sitekey, (token) => {
        clear();
        say({ is: "token", token });
      }),
    cancel: clear,
    send,
  };
  say({ is: "ready" });
})();
