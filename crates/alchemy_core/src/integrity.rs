use sha2::{Digest, Sha256, Sha512};

/// Verify integrity of data against an SRI hash string.
/// Supports "sha512-<base64>" and "sha256-<base64>" formats.
/// Returns false for unsupported or malformed hash types (fail closed).
pub fn verify_integrity(data: &[u8], expected: &str) -> bool {
    if let Some(hash) = expected.strip_prefix("sha512-") {
        let computed = base64_encode(&Sha512::digest(data));
        computed == hash
    } else if let Some(hash) = expected.strip_prefix("sha256-") {
        let computed = base64_encode(&Sha256::digest(data));
        computed == hash
    } else {
        false
    }
}

/// Compute SHA-512 integrity in SRI format (preferred; matches npm default).
pub fn compute_integrity(data: &[u8]) -> String {
    format!("sha512-{}", base64_encode(&Sha512::digest(data)))
}

/// Compute SHA-256 integrity in SRI format.
pub fn compute_integrity_sha256(data: &[u8]) -> String {
    format!("sha256-{}", base64_encode(&Sha256::digest(data)))
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;

        result.push(CHARS[((n >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((n >> 12) & 0x3F) as usize] as char);

        if chunk.len() > 1 {
            result.push(CHARS[((n >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }

        if chunk.len() > 2 {
            result.push(CHARS[(n & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_verify_correct() {
        let data = b"hello world";
        let sri = compute_integrity_sha256(data);
        assert!(verify_integrity(data, &sri));
    }

    #[test]
    fn test_sha256_verify_wrong_data() {
        let data = b"hello world";
        let sri = compute_integrity_sha256(data);
        assert!(!verify_integrity(b"different data", &sri));
    }

    #[test]
    fn test_sha512_verify_correct() {
        let data = b"hello world";
        let sri = compute_integrity(data);
        assert!(verify_integrity(data, &sri));
    }

    #[test]
    fn test_sha512_verify_wrong_data() {
        let data = b"hello world";
        let sri = compute_integrity(data);
        assert!(!verify_integrity(b"different data", &sri));
    }

    #[test]
    fn test_unknown_hash_type_returns_false() {
        // Unknown hash type must fail closed (security: don't silently skip)
        assert!(!verify_integrity(b"data", "md5-somehash"));
        assert!(!verify_integrity(b"data", "sha1-somehash"));
        assert!(!verify_integrity(b"data", "invalid"));
    }
}
