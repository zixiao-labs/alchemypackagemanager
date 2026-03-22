use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Alchemy package manager configuration.
/// Loaded from .npmrc files with cascading priority:
/// project .npmrc > user ~/.npmrc > global /etc/npmrc
#[derive(Debug, Clone)]
pub struct AlchemyConfig {
    pub default_registry: String,
    pub scoped_registries: BTreeMap<String, String>,
    pub auth_tokens: BTreeMap<String, String>,
    pub proxy: Option<String>,
    pub https_proxy: Option<String>,
    pub strict_ssl: bool,
    pub auto_install_peers: bool,
    pub ignore_scripts: bool,
    pub store_dir: PathBuf,
    pub metadata_cache_ttl: u64,
}

impl Default for AlchemyConfig {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        Self {
            default_registry: "https://registry.npmjs.org".to_string(),
            scoped_registries: BTreeMap::new(),
            auth_tokens: BTreeMap::new(),
            proxy: None,
            https_proxy: None,
            strict_ssl: true,
            auto_install_peers: true,
            ignore_scripts: false,
            store_dir: PathBuf::from(home).join(".alchemy-store"),
            metadata_cache_ttl: 300,
        }
    }
}

impl AlchemyConfig {
    /// Load config from the current directory's project context.
    pub fn load() -> crate::error::AlchemyResult<Self> {
        let cwd = std::env::current_dir()?;
        Self::load_from_dir(&cwd)
    }

    /// Load config with cascading .npmrc files relative to the given project dir.
    pub fn load_from_dir(project_dir: &Path) -> crate::error::AlchemyResult<Self> {
        let mut config = Self::default();

        // Global /etc/npmrc (lowest priority)
        let global_rc = PathBuf::from("/etc/npmrc");
        if global_rc.is_file() {
            if let Ok(content) = std::fs::read_to_string(&global_rc) {
                config.apply_npmrc(&content);
            }
        }

        // User ~/.npmrc
        if let Ok(home) = std::env::var("HOME") {
            let user_rc = PathBuf::from(&home).join(".npmrc");
            if user_rc.is_file() {
                if let Ok(content) = std::fs::read_to_string(&user_rc) {
                    config.apply_npmrc(&content);
                }
            }
        }

        // Project .npmrc (highest priority)
        let project_rc = project_dir.join(".npmrc");
        if project_rc.is_file() {
            if let Ok(content) = std::fs::read_to_string(&project_rc) {
                config.apply_npmrc(&content);
            }
        }

        // Environment variable overrides
        if let Ok(val) = std::env::var("NPM_CONFIG_REGISTRY") {
            config.default_registry = val;
        }
        if let Ok(val) = std::env::var("HTTP_PROXY") {
            config.proxy = Some(val);
        }
        if let Ok(val) = std::env::var("HTTPS_PROXY") {
            config.https_proxy = Some(val);
        }

        Ok(config)
    }

    /// Apply settings from an .npmrc file content.
    fn apply_npmrc(&mut self, content: &str) {
        for line in content.lines() {
            let line = line.trim();

            // Skip comments and empty lines
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }

            // Auth token lines: //registry.example.com/:_authToken=TOKEN
            if line.starts_with("//") {
                if let Some((registry_part, token)) = line.split_once(":_authToken=") {
                    let registry_url = format!("https:{}", registry_part);
                    let token = interpolate_env(token);
                    self.auth_tokens.insert(registry_url, token);
                    continue;
                }
                // _auth (base64) lines — also supported
                if let Some((registry_part, auth)) = line.split_once(":_auth=") {
                    let registry_url = format!("https:{}", registry_part);
                    let auth = interpolate_env(auth);
                    self.auth_tokens
                        .insert(registry_url, format!("Basic {}", auth));
                    continue;
                }
            }

            // Key=value lines
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = interpolate_env(value.trim());

                match key {
                    "registry" => self.default_registry = value,
                    "proxy" => self.proxy = Some(value),
                    "https-proxy" => self.https_proxy = Some(value),
                    "strict-ssl" => self.strict_ssl = value != "false",
                    "auto-install-peers" => self.auto_install_peers = value != "false",
                    "ignore-scripts" => self.ignore_scripts = value == "true",
                    "cache-ttl" => {
                        if let Ok(ttl) = value.parse::<u64>() {
                            self.metadata_cache_ttl = ttl;
                        }
                    }
                    _ => {
                        // Scoped registry: @scope:registry=URL
                        if let Some(scope_key) = key.strip_prefix('@') {
                            if let Some((scope, sub_key)) = scope_key.split_once(':') {
                                if sub_key == "registry" {
                                    self.scoped_registries.insert(scope.to_string(), value);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Get the registry URL for a given package name (respecting scoped registries).
    pub fn registry_for_package(&self, name: &str) -> &str {
        if let Some(scope) = name.strip_prefix('@') {
            if let Some(scope_name) = scope.split('/').next() {
                if let Some(url) = self.scoped_registries.get(scope_name) {
                    return url;
                }
            }
        }
        &self.default_registry
    }

    /// Get the auth token for a given registry URL.
    pub fn auth_for_registry(&self, registry_url: &str) -> Option<&str> {
        // Try exact match
        if let Some(token) = self.auth_tokens.get(registry_url) {
            return Some(token);
        }
        // Try matching by stripping trailing slash
        let normalized = registry_url.trim_end_matches('/');
        if let Some(token) = self.auth_tokens.get(normalized) {
            return Some(token);
        }
        None
    }
}

/// Interpolate `${ENV_VAR}` references in a value string.
fn interpolate_env(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '$' && chars.peek() == Some(&'{') {
            chars.next(); // consume '{'
            let mut var_name = String::new();
            for c in chars.by_ref() {
                if c == '}' {
                    break;
                }
                var_name.push(c);
            }
            if let Ok(val) = std::env::var(&var_name) {
                result.push_str(&val);
            }
        } else {
            result.push(c);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AlchemyConfig::default();
        assert_eq!(config.default_registry, "https://registry.npmjs.org");
        assert!(config.auto_install_peers);
        assert!(!config.ignore_scripts);
        assert!(config.strict_ssl);
    }

    #[test]
    fn test_parse_npmrc() {
        let mut config = AlchemyConfig::default();
        config.apply_npmrc(
            r#"
# Comment line
registry=https://custom.registry.com
@myorg:registry=https://npm.myorg.com
//npm.myorg.com/:_authToken=secret-token
auto-install-peers=false
ignore-scripts=true
proxy=http://proxy.example.com:8080
"#,
        );

        assert_eq!(config.default_registry, "https://custom.registry.com");
        assert_eq!(
            config.scoped_registries.get("myorg").unwrap(),
            "https://npm.myorg.com"
        );
        assert_eq!(
            config.auth_tokens.get("https://npm.myorg.com/").unwrap(),
            "secret-token"
        );
        assert!(!config.auto_install_peers);
        assert!(config.ignore_scripts);
        assert_eq!(
            config.proxy.as_deref(),
            Some("http://proxy.example.com:8080")
        );
    }

    #[test]
    fn test_registry_for_package() {
        let mut config = AlchemyConfig::default();
        config
            .scoped_registries
            .insert("myorg".to_string(), "https://npm.myorg.com".to_string());

        assert_eq!(
            config.registry_for_package("@myorg/pkg"),
            "https://npm.myorg.com"
        );
        assert_eq!(
            config.registry_for_package("express"),
            "https://registry.npmjs.org"
        );
    }
}
