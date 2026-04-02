// Re-export integrity functions from alchemy_core for backward compatibility.
// The canonical implementations live in alchemy_core::integrity.
pub use alchemy_core::integrity::{compute_integrity, compute_integrity_sha256, verify_integrity};
