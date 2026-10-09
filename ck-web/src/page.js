// Run in a page on boards.4chan.org (its robots.txt: no ads, no scripts of its own), as
// 4chan's own reply form does it: the captcha comes from a frame on sys.4chan.org, which
// passes it up with postMessage, and the post is sent with the site's cookies. What it
// finds goes to ck-web on the console, as "ck-web:" and JSON.
(() => {
  if (window.ckweb) return;
  const say = (m) => console.log("ck-web:" + JSON.stringify(m));
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

  const text = (html) => new DOMParser().parseFromString(String(html), "text/html").body.textContent.trim();

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

  // 4chan asks for an hCaptcha first (to earn a ticket) when it's been busy: shown in the
  // browser view for a person to do, then the captcha is asked for again with its answer.
  const hcaptcha = (sitekey) => {
    const box = document.createElement("div");
    box.style = "display:flex;justify-content:center;padding-top:16px";
    sheet().appendChild(box);
    window.ckwebHcaptcha = () => window.hcaptcha.render(box, { sitekey, callback: (resp) => load(resp) });
    if (window.hcaptcha) {
      window.ckwebHcaptcha();
    } else {
      const s = document.createElement("script");
      s.src = "https://js.hcaptcha.com/1/api.js?onload=ckwebHcaptcha&render=explicit&recaptchacompat=off";
      s.onerror = () => say({ is: "failed", error: "Couldn't load hCaptcha" });
      document.head.appendChild(s);
    }
    say({ is: "human" });
  };

  window.addEventListener("message", (e) => {
    if (e.origin !== SYS || !e.data || !e.data.twister) return;
    const t = e.data.twister;
    if (t.ticket) localStorage.setItem(TICKET, t.ticket);
    else if (t.ticket === false) localStorage.removeItem(TICKET);
    if (t.mpcd) return hcaptcha(t.sitekey);
    clear();
    say({ is: "captcha", twister: t });
  });

  const post = async (board, thread, fields, file) => {
    const form = new FormData();
    form.append("mode", "regist");
    if (thread > 0) form.append("resto", String(thread));
    for (const [k, v] of fields) form.append(k, v);
    if (file) {
      const bytes = Uint8Array.from(atob(file.data), (c) => c.charCodeAt(0));
      form.append("upfile", new Blob([bytes], { type: file.mime }), file.name);
    }
    let r, body;
    try {
      r = await fetch(SYS + "/" + encodeURIComponent(board) + "/post", {
        method: "POST",
        body: form,
        credentials: "include",
        headers: { Accept: "application/json" },
      });
      body = await r.text();
    } catch (e) {
      return say({ is: "failed", error: "Couldn't reach 4chan: " + e });
    }
    if ((r.headers.get("Content-Type") || "").includes("application/json")) {
      let j;
      try {
        j = JSON.parse(body);
      } catch (e) {
        return say({ is: "failed", error: "4chan's answer didn't make sense" });
      }
      if (j.error) return say({ is: "failed", error: text(j.error) });
      if (j.pid) return say({ is: "posted", thread: Number(j.tid) || Number(j.pid), no: Number(j.pid) });
    }
    const err = body.match(/"errmsg"[^>]*>(.*?)<\/span/);
    if (err) return say({ is: "failed", error: text(err[1]) });
    const ok = body.match(/<!-- thread:([0-9]+),no:([0-9]+) -->/);
    if (ok) return say({ is: "posted", thread: Number(ok[1]) || Number(ok[2]), no: Number(ok[2]) });
    say({ is: "failed", error: "4chan answered " + r.status + " and didn't say whether it posted" });
  };

  window.ckweb = {
    captcha: (board, thread) => {
      wanted = { board, thread };
      load(null);
    },
    cancel: clear,
    post,
  };
  say({ is: "ready" });
})();
