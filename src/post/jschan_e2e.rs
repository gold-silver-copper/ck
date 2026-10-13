//! jschan accounts, end to end: the real ck-web, logging in to real jschan servers and posting
//! there, as ck does. The servers are local, started and seeded by `e2e/jschan/servers.sh`
//! (upstream jschan, and ptchina, ptchan.org's fork), and CI runs this on Linux, macOS and
//! Windows (`.github/workflows/e2e-accounts.yml`):
//!
//!   e2e/jschan/servers.sh install && e2e/jschan/servers.sh start
//!   CK_WEB=target/debug/ck-web CK_WEB_PROFILE=<an empty folder> \
//!     CK_E2E_JSCHAN="http://127.0.0.1:7100 http://localhost:7101" \
//!     cargo test -- --ignored jschan_accounts --nocapture
//!
//! Each site is on a host of its own (127.0.0.1, localhost): cookies are kept by host, not
//! port, so two sites on one host would share a login. ck-web's profile should start empty.

// Counting and a hash's bytes, in a test.
#![allow(clippy::arithmetic_side_effects)]

use std::cell::Cell;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use sha2::{Digest, Sha256};

use super::jschan::{Account, Jschan};
use super::{Ask, Draft, Login, Poster, Session, Where, Work};
use crate::captcha::Challenge;
use crate::web::Helper;

/// The seeded accounts' password and the two-factor account's secret (`e2e/jschan/seed.js`).
const PASSWORD: &str = "ck-e2e-password";
const TWOFACTOR: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

/// Someone who won't do a captcha: it counts the ones shown, and stops the post.
#[derive(Default)]
struct NoCaptcha {
    shown: Cell<usize>,
}

impl Ask for NoCaptcha {
    fn solve(&self, _challenge: &Challenge) -> Result<Option<String>> {
        self.shown.set(self.shown.get() + 1);
        bail!("stopped at the captcha")
    }

    fn wait(&self, _until: Instant, message: &str) -> Result<()> {
        bail!("the site said to wait: {message}")
    }
}

#[test]
#[ignore = "needs ck-web and the local jschan servers (e2e/jschan/servers.sh)"]
fn jschan_accounts_e2e() {
    let ck_web = std::env::var("CK_WEB").expect("CK_WEB: where the ck-web to test is");
    let sites = std::env::var("CK_E2E_JSCHAN").expect("CK_E2E_JSCHAN: the servers' URLs");
    let roots: Vec<&str> = sites.split_whitespace().collect();
    let helper = Helper::start(std::path::Path::new(&ck_web), |_| {}, |last| eprintln!("ck-web stopped: {last:?}")).unwrap();
    for (i, root) in roots.iter().enumerate() {
        eprintln!("{root}");
        let site = Site::new(root);
        site.open(&helper);
        assert!(!site.account.logged_in(&helper).unwrap(), "{root}: logged in before logging in");

        // Refused: a wrong password, and the two-factor account without its code or with a
        // wrong one. jschan says the same to each.
        let wrong = site.log_in(&helper, "ckplain", "not-the-password", "").unwrap_err().to_string();
        assert!(wrong.starts_with("Incorrect login credentials") && wrong.contains("2FA code"), "{wrong}");
        let no_code = site.log_in(&helper, "cktwofa", PASSWORD, "").unwrap_err().to_string();
        assert!(no_code.contains("2FA code"), "{no_code}");
        let code = totp(TWOFACTOR, now());
        let wrong_code = format!("{:06}", (code.parse::<u32>().unwrap() + 1) % 1_000_000);
        assert_eq!(site.log_in(&helper, "cktwofa", PASSWORD, &wrong_code).unwrap_err().to_string(), "Incorrect login credentials");
        assert!(!site.account.logged_in(&helper).unwrap());
        // Its code: in, and out again.
        site.log_in(&helper, "cktwofa", PASSWORD, &code).unwrap();
        assert!(site.account.logged_in(&helper).unwrap());
        site.account.log_out(&helper).unwrap();
        assert!(!site.account.logged_in(&helper).unwrap());

        // Logged out: posts where there's no captcha, asked one where there is.
        site.post(&helper, "open", "anonymous, no captcha").unwrap();
        assert_eq!(site.post_stopped_at_captcha(&helper, "capt"), 1);

        // A plain account: the captcha all the same (tried without one first).
        site.log_in(&helper, "ckplain", PASSWORD, "").unwrap();
        site.post(&helper, "open", "plain account, no captcha").unwrap();
        assert_eq!(site.post_stopped_at_captcha(&helper, "capt"), 1);
        site.account.log_out(&helper).unwrap();

        // An account let off the captcha (ptchan's gold): none asked.
        site.log_in(&helper, "ckgold", PASSWORD, "").unwrap();
        site.post(&helper, "capt", "gold account, no captcha").unwrap();
        // Each site has its own login: not the next one.
        if let Some(other) = roots.get(i + 1).map(|r| Site::new(r)) {
            other.open(&helper);
            assert!(!other.account.logged_in(&helper).unwrap(), "{} took {root}'s login", other.root);
            site.open(&helper);
            assert!(site.account.logged_in(&helper).unwrap());
        }
        site.account.log_out(&helper).unwrap();
        assert!(!site.account.logged_in(&helper).unwrap());
    }
}

/// One server: its poster and account.
struct Site {
    root: String,
    jschan: Jschan,
    account: Account,
}

impl Site {
    fn new(root: &str) -> Self {
        let root = root.trim_end_matches('/').to_string();
        Site { jschan: Jschan { root: root.clone(), site: "e2e".into() }, account: Account { root: root.clone(), site: "e2e".into() }, root }
    }

    fn open(&self, helper: &Helper) {
        crate::web::Web::open(helper, &self.account.page()).unwrap();
    }

    fn log_in(&self, helper: &Helper, username: &str, password: &str, code: &str) -> Result<()> {
        self.account.log_in(helper, &[username.into(), password.into(), code.into()])
    }

    fn post(&self, helper: &Helper, board: &str, comment: &str) -> Result<()> {
        let ask = NoCaptcha::default();
        let draft = Draft { comment: format!("{comment} ({})", now()), password: "ck-e2e".into(), ..Default::default() };
        let to = Where { site: "e2e".into(), board: board.into(), thread: 0 };
        let posted = self.jschan.post(&Session { web: helper, ask: &ask, work: Work::new(None, None) }, &to, &draft)?;
        assert!(posted.no > 0 && ask.shown.get() == 0, "{posted:?}, {} captchas", ask.shown.get());
        Ok(())
    }

    /// Post on `board` refusing any captcha: how many were shown.
    fn post_stopped_at_captcha(&self, helper: &Helper, board: &str) -> usize {
        let ask = NoCaptcha::default();
        let draft = Draft { comment: format!("never sent ({})", now()), password: "ck-e2e".into(), ..Default::default() };
        let to = Where { site: "e2e".into(), board: board.into(), thread: 0 };
        let r = self.jschan.post(&Session { web: helper, ask: &ask, work: Work::new(None, None) }, &to, &draft);
        assert!(r.is_err(), "posted on /{board}/ without a captcha");
        ask.shown.get()
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// jschan's two-factor code at `unix` seconds: TOTP with SHA-256, six digits, 30 seconds a
/// step (its `otpauth` defaults, but the hash), from a base32 secret.
fn totp(secret: &str, unix: u64) -> String {
    let key = base32(secret);
    let mac = hmac_sha256(&key, &(unix / 30).to_be_bytes());
    let at = usize::from(mac[31] & 0x0f);
    let n = u32::from_be_bytes([mac[at] & 0x7f, mac[at + 1], mac[at + 2], mac[at + 3]]);
    format!("{:06}", n % 1_000_000)
}

fn base32(s: &str) -> Vec<u8> {
    let (mut bits, mut acc, mut out) = (0u32, 0u32, Vec::new());
    for c in s.bytes().filter(|&c| c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'2'..=b'7' => c - b'2' + 26,
            _ => panic!("not base32: {s}"),
        };
        acc = (acc << 5) | u32::from(v);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    block[..key.len()].copy_from_slice(key);
    let pad = |byte: u8| block.map(|b| b ^ byte);
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(message).finalize();
    Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize().into()
}

#[test]
fn two_factor_codes_are_jschans() {
    // RFC 6238's SHA-256 test vector, its 32-byte key in base32, cut to six digits.
    let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZA";
    assert_eq!(totp(secret, 59), "119246");
    assert_eq!(totp(secret, 1_111_111_109), "084774");
}
