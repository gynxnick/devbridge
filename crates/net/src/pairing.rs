//! Pairing cryptography.
//!
//! 1. The receiving device shows a single-use 8-character code (40 bits).
//! 2. Both devices run SPAKE2 with the code as password. An eavesdropper learns
//!    nothing; an active attacker gets one online guess per attempt.
//! 3. Each side MACs the two TLS certificate fingerprints *as it observed
//!    them* with the PAKE key. A man in the middle presents different
//!    certificates to each side, so the MACs can't match.
//! 4. Both screens show a short verification code derived from the key.

use data_encoding::HEXLOWER;
use rand::RngExt;
use ring::hmac;
use spake2::{Ed25519Group, Identity as PakeId, Password, Spake2};

/// Crockford base32 alphabet (no I, L, O, U).
const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const PAKE_ID: &[u8] = b"devbridge-pair-v1";

/// A fresh pairing code like `7KQ2-M9XD`.
pub fn new_code() -> String {
    let mut rng = rand::rng();
    let chars: String = (0..8).map(|_| ALPHABET[rng.random_range(0..32)] as char).collect();
    format!("{}-{}", &chars[..4], &chars[4..])
}

/// Canonical form of user input: uppercase, separators removed, ambiguous
/// characters mapped. Returns `None` unless exactly 8 valid characters remain.
pub fn normalize_code(input: &str) -> Option<String> {
    let s: String = input
        .chars()
        .filter(|c| !matches!(c, '-' | ' ' | '_'))
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect();
    (s.len() == 8 && s.bytes().all(|b| ALPHABET.contains(&b))).then_some(s)
}

pub struct Pake(Spake2<Ed25519Group>);

pub fn pake_start(code: &str) -> (Pake, Vec<u8>) {
    let canonical = normalize_code(code).unwrap_or_default();
    let (s, msg) = Spake2::<Ed25519Group>::start_symmetric(&Password::new(canonical.as_bytes()), &PakeId::new(PAKE_ID));
    (Pake(s), msg)
}

#[derive(Clone)]
pub struct PairKey(Vec<u8>);

impl PairKey {
    fn hmac_key(&self) -> hmac::Key {
        hmac::Key::new(hmac::HMAC_SHA256, &self.0)
    }

    fn input(role: &str, server_fp: &str, client_fp: &str) -> Vec<u8> {
        format!("devbridge-pair-v1|{role}|{server_fp}|{client_fp}").into_bytes()
    }

    /// MAC proving knowledge of the key, bound to both fingerprints.
    pub fn mac(&self, role: &str, server_fp: &str, client_fp: &str) -> String {
        HEXLOWER.encode(hmac::sign(&self.hmac_key(), &Self::input(role, server_fp, client_fp)).as_ref())
    }

    /// Constant-time verification.
    pub fn verify(&self, role: &str, server_fp: &str, client_fp: &str, mac_hex: &str) -> bool {
        let Ok(tag) = HEXLOWER.decode(mac_hex.as_bytes()) else { return false };
        hmac::verify(&self.hmac_key(), &Self::input(role, server_fp, client_fp), &tag).is_ok()
    }

    /// Six-digit code shown on both screens, e.g. `482 913`.
    pub fn verification_code(&self) -> String {
        let t = hmac::sign(&self.hmac_key(), b"devbridge-pair-v1|sas");
        let n = u32::from_be_bytes(t.as_ref()[..4].try_into().expect("4 bytes")) % 1_000_000;
        format!("{:03} {:03}", n / 1000, n % 1000)
    }
}

pub fn pake_finish(p: Pake, peer_msg: &[u8]) -> Option<PairKey> {
    p.0.finish(peer_msg).ok().map(PairKey)
}

pub const ROLE_CLIENT: &str = "client";
pub const ROLE_SERVER: &str = "server";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_well_formed_and_normalize() {
        let c = new_code();
        assert_eq!(c.len(), 9);
        assert!(normalize_code(&c).is_some());
        assert_eq!(normalize_code("7kq2 m9xd").as_deref(), Some("7KQ2M9XD"));
        assert_eq!(normalize_code("OIL0-0000").as_deref(), Some("01100000"));
        assert_eq!(normalize_code("7KQ2-M9X"), None);
        assert_eq!(normalize_code("7KQ2-M9XU"), None, "U is not in the alphabet");
    }

    #[test]
    fn same_code_agrees_and_binds_fingerprints() {
        let (a, ma) = pake_start("7KQ2-M9XD");
        let (b, mb) = pake_start("7kq2m9xd");
        let ka = pake_finish(a, &mb).unwrap();
        let kb = pake_finish(b, &ma).unwrap();
        assert_eq!(ka.verification_code(), kb.verification_code());
        let mac = ka.mac(ROLE_CLIENT, "S", "C");
        assert!(kb.verify(ROLE_CLIENT, "S", "C", &mac));
        assert!(!kb.verify(ROLE_SERVER, "S", "C", &mac), "role is bound");
        assert!(!kb.verify(ROLE_CLIENT, "S-mitm", "C", &mac), "fingerprints are bound");
        assert!(!kb.verify(ROLE_CLIENT, "S", "C", "zz"));
    }

    #[test]
    fn wrong_code_fails_verification() {
        let (a, ma) = pake_start("7KQ2-M9XD");
        let (b, mb) = pake_start("7KQ2-M9XE");
        let ka = pake_finish(a, &mb).unwrap();
        let kb = pake_finish(b, &ma).unwrap();
        assert!(!kb.verify(ROLE_CLIENT, "S", "C", &ka.mac(ROLE_CLIENT, "S", "C")));
    }
}
