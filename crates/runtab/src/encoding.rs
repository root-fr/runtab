use sha2::{Digest, Sha256};

const HEX: &[u8; 16] = b"0123456789abcdef";
const B64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Lowercase hex of arbitrary bytes.
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// SHA-256 of the joined parts (separated by `\x1f`), lowercase hex. Used for
/// the deterministic `event_id` and the opaque `session_id` that leaves the
/// machine — the raw session id never syncs.
pub fn sha256_hex(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update([0x1f]);
        }
        h.update(p.as_bytes());
    }
    hex(&h.finalize())
}

/// URL-safe base64 without padding.
pub fn base64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64URL[(n >> 18 & 0x3f) as usize] as char);
        out.push(B64URL[(n >> 12 & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(B64URL[(n >> 6 & 0x3f) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(B64URL[(n & 0x3f) as usize] as char);
        }
    }
    out
}

/// Fill a buffer with OS random bytes, returning whether the OS RNG succeeded.
fn os_random(buf: &mut [u8]) -> bool {
    getrandom::getrandom(buf).is_ok()
}

/// Fill a buffer with OS random bytes, falling back to a time-seeded value only
/// if the OS RNG is unavailable. Used solely for the non-secret machine id (a
/// public identifier sent in every payload), never for a secret.
fn random_bytes(buf: &mut [u8]) {
    if !os_random(buf) {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (seed >> (i % 16 * 8)) as u8 ^ (i as u8).wrapping_mul(31);
        }
    }
}

/// A client `device_code`: `dc_` + 32 base64url chars (24 random bytes → 192
/// bits, exceeding the ≥128-bit floor the hardening addendum requires). The
/// device_code is the sole secret gating token issuance, so it fails closed:
/// if the OS RNG is unavailable we error rather than mint a guessable code.
pub fn new_device_code() -> anyhow::Result<String> {
    let mut raw = [0u8; 24];
    if !os_random(&mut raw) {
        anyhow::bail!("OS RNG unavailable; refusing to mint a weak device code");
    }
    Ok(format!("dc_{}", base64url(&raw)))
}

/// A random integer in `[0, bound)`, falling back to a time-seeded value if
/// the OS RNG is unavailable. Non-secret jitter only (e.g. the cron
/// minute-offset phase in `sync auto on`) — never use this for a secret.
pub fn random_u32(bound: u32) -> u32 {
    if bound == 0 {
        return 0;
    }
    let mut b = [0u8; 4];
    random_bytes(&mut b);
    u32::from_le_bytes(b) % bound
}

/// A random UUID-v4 string for the stable per-machine id (non-secret).
pub fn new_uuid() -> String {
    let mut b = [0u8; 16];
    random_bytes(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex(&b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}
