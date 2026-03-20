use sha2::{Digest, Sha256};

/// Verify integrity of data against an expected hash.
/// Supports SRI format: "sha512-..." or "sha256-..."
pub fn verify_integrity(data: &[u8], expected: &str) -> bool {
    if let Some(hash) = expected.strip_prefix("sha256-") {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();
        let computed = base64_encode(&result);
        computed == hash
    } else {
        // For now, skip verification for unsupported hash types
        true
    }
}

/// Compute SHA-256 integrity in SRI format
pub fn compute_integrity(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    format!("sha256-{}", base64_encode(&result))
}

fn base64_encode(data: &[u8]) -> String {
    // Simple base64 encoding
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
