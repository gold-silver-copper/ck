//! Proofs of work some sites ask before a post (LynxChan's block bypass, kohlchan's
//! "hashcash"): the number that fits, searched on a few threads for a while (`Work`, set by
//! `proof_of_work_seconds` and `proof_of_work_threads`).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How hard a proof of work is searched: for how long, on how many threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Work {
    pub seconds: u64,
    pub threads: usize,
}

impl Work {
    /// As set, or three minutes, on half the machine's threads (four at most).
    pub fn new(seconds: Option<u64>, threads: Option<usize>) -> Self {
        Work { seconds: seconds.unwrap_or(180), threads: threads.unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| (n.get() / 2).clamp(1, 4))) }
    }
}

/// The first number under `limit` that `fits` (or none, also once `work`'s time is up).
pub fn search(work: Work, limit: u64, fits: impl Fn(u64) -> bool + Sync) -> Option<u64> {
    let until = Instant::now().checked_add(Duration::from_secs(work.seconds));
    let (next, found, done) = (AtomicU64::new(0), AtomicU64::new(u64::MAX), AtomicBool::new(false));
    std::thread::scope(|s| {
        for _ in 0..work.threads.max(1) {
            s.spawn(|| {
                while !done.load(Ordering::Relaxed) && until.is_none_or(|t| Instant::now() < t) {
                    let n = next.fetch_add(1, Ordering::Relaxed);
                    if n >= limit {
                        break;
                    }
                    if fits(n) {
                        found.fetch_min(n, Ordering::Relaxed);
                        done.store(true, Ordering::Relaxed);
                    }
                }
            });
        }
    });
    Some(found.into_inner()).filter(|&n| n != u64::MAX)
}

/// LynxChan's block bypass validation: the number (as text, the salt) whose PBKDF2-SHA512 of
/// `session` (16384 rounds, 256 bytes), in base64, is `hash`.
pub fn lynxchan_bypass(work: Work, session: &str, hash: &str) -> Option<u64> {
    use base64::Engine;
    search(work, u64::MAX, |n| {
        let mut out = [0u8; 256];
        pbkdf2::pbkdf2_hmac::<sha2::Sha512>(session.as_bytes(), n.to_string().as_bytes(), 16384, &mut out);
        base64::engine::general_purpose::STANDARD.encode(out) == hash
    })
}

/// kohlchan's hashcash: the number under `difficulty` that an encoded argon2 hash is of.
pub fn argon2_secret(work: Work, encoded: &str, difficulty: u64) -> Option<u64> {
    use argon2::{PasswordHash, PasswordVerifier};
    let hash = PasswordHash::new(encoded).ok()?;
    search(work, difficulty, |n| argon2::Argon2::default().verify_password(n.to_string().as_bytes(), &hash).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_numbers_that_fit_are_found() {
        let work = Work { seconds: 60, threads: 2 };
        assert_eq!(search(work, 100, |n| n == 37), Some(37));
        assert_eq!(search(work, 10, |n| n == 37), None);
        assert_eq!(search(Work { seconds: 0, ..work }, 100, |n| n == 37), None);
        use base64::Engine;
        let mut out = [0u8; 256];
        pbkdf2::pbkdf2_hmac::<sha2::Sha512>(b"sess", b"3", 16384, &mut out);
        assert_eq!(lynxchan_bypass(work, "sess", &base64::engine::general_purpose::STANDARD.encode(out)), Some(3));
    }
}
