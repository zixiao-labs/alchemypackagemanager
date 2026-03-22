//! Platform detection and compatibility checking for npm packages.
//! Maps Rust's platform constants to npm naming conventions.

/// Current platform information
pub struct Platform {
    pub os: String,
    pub cpu: String,
}

impl Platform {
    /// Detect the current platform
    pub fn current() -> Self {
        Self {
            os: map_os(std::env::consts::OS),
            cpu: map_cpu(std::env::consts::ARCH),
        }
    }

    /// Check if the current OS matches the package's os constraints.
    /// Empty constraints = no restriction. Supports negation with "!" prefix.
    pub fn matches_os(&self, constraints: &[String]) -> bool {
        matches_constraint(&self.os, constraints)
    }

    /// Check if the current CPU matches the package's cpu constraints.
    /// Empty constraints = no restriction. Supports negation with "!" prefix.
    pub fn matches_cpu(&self, constraints: &[String]) -> bool {
        matches_constraint(&self.cpu, constraints)
    }

    /// Check both os and cpu constraints at once.
    pub fn is_compatible(&self, os: Option<&Vec<String>>, cpu: Option<&Vec<String>>) -> bool {
        if let Some(os_constraints) = os {
            if !os_constraints.is_empty() && !self.matches_os(os_constraints) {
                return false;
            }
        }
        if let Some(cpu_constraints) = cpu {
            if !cpu_constraints.is_empty() && !self.matches_cpu(cpu_constraints) {
                return false;
            }
        }
        true
    }
}

/// Map Rust OS name to npm OS name
fn map_os(rust_os: &str) -> String {
    match rust_os {
        "macos" => "darwin",
        "windows" => "win32",
        "linux" => "linux",
        "freebsd" => "freebsd",
        "openbsd" => "openbsd",
        "android" => "android",
        "solaris" | "illumos" => "sunos",
        other => other,
    }
    .to_string()
}

/// Map Rust CPU architecture to npm CPU name
fn map_cpu(rust_arch: &str) -> String {
    match rust_arch {
        "x86_64" => "x64",
        "x86" => "ia32",
        "aarch64" => "arm64",
        "arm" => "arm",
        "powerpc64" => "ppc64",
        "s390x" => "s390x",
        "mips" => "mips",
        "mips64" => "mips64el",
        other => other,
    }
    .to_string()
}

/// Check if a value matches a list of constraints.
/// - Empty list = no constraint (always matches)
/// - `["linux", "darwin"]` = must be one of these
/// - `["!win32"]` = must NOT be win32
fn matches_constraint(value: &str, constraints: &[String]) -> bool {
    if constraints.is_empty() {
        return true;
    }

    let mut has_positive = false;
    let mut positive_match = false;

    for constraint in constraints {
        if let Some(negated) = constraint.strip_prefix('!') {
            // Negation: if the value matches a negated constraint, it fails
            if negated == value {
                return false;
            }
        } else {
            has_positive = true;
            if constraint == value {
                positive_match = true;
            }
        }
    }

    // If there are positive constraints, the value must match at least one
    if has_positive {
        return positive_match;
    }

    // Only negation constraints, and none matched = OK
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_platform() {
        let platform = Platform::current();
        // Just verify it doesn't panic and returns non-empty strings
        assert!(!platform.os.is_empty());
        assert!(!platform.cpu.is_empty());
    }

    #[test]
    fn test_matches_empty_constraint() {
        assert!(matches_constraint("darwin", &[]));
    }

    #[test]
    fn test_matches_positive() {
        let constraints = vec!["linux".to_string(), "darwin".to_string()];
        assert!(matches_constraint("darwin", &constraints));
        assert!(matches_constraint("linux", &constraints));
        assert!(!matches_constraint("win32", &constraints));
    }

    #[test]
    fn test_matches_negation() {
        let constraints = vec!["!win32".to_string()];
        assert!(matches_constraint("darwin", &constraints));
        assert!(matches_constraint("linux", &constraints));
        assert!(!matches_constraint("win32", &constraints));
    }

    #[test]
    fn test_os_mapping() {
        assert_eq!(map_os("macos"), "darwin");
        assert_eq!(map_os("windows"), "win32");
        assert_eq!(map_os("linux"), "linux");
    }

    #[test]
    fn test_cpu_mapping() {
        assert_eq!(map_cpu("x86_64"), "x64");
        assert_eq!(map_cpu("aarch64"), "arm64");
        assert_eq!(map_cpu("x86"), "ia32");
    }
}
