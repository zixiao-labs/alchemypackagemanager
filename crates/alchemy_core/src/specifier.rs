use std::path::PathBuf;

/// Parsed dependency specifier from package.json
#[derive(Debug, Clone)]
pub enum DependencySpecifier {
    /// A semver range or tag from registry (e.g., "^1.2.3", "latest", "*")
    Registry(String),
    /// A git repository (e.g., "github:user/repo#tag", "git+https://...")
    Git(GitSpec),
    /// A tarball URL (e.g., "https://example.com/pkg.tgz")
    Url(String),
    /// A local file path (e.g., "file:../local-pkg")
    Path(PathBuf),
    /// An aliased package (e.g., "npm:other-pkg@^1.0")
    Alias {
        package: String,
        spec: Box<DependencySpecifier>,
    },
    /// A workspace reference (e.g., "workspace:*", "workspace:^")
    Workspace(String),
}

/// Git dependency specification
#[derive(Debug, Clone)]
pub struct GitSpec {
    pub url: String,
    pub commitish: Option<String>,
}

/// Parse a dependency specifier string into a structured type.
pub fn parse(spec: &str) -> DependencySpecifier {
    let spec = spec.trim();

    // workspace: protocol
    if let Some(rest) = spec.strip_prefix("workspace:") {
        return DependencySpecifier::Workspace(rest.to_string());
    }

    // npm: alias prefix
    if let Some(rest) = spec.strip_prefix("npm:") {
        // "npm:other-pkg@^1.0" → Alias { package: "other-pkg", spec: Registry("^1.0") }
        if let Some((name, version)) = split_alias(rest) {
            return DependencySpecifier::Alias {
                package: name.to_string(),
                spec: Box::new(parse(version)),
            };
        }
        return DependencySpecifier::Alias {
            package: rest.to_string(),
            spec: Box::new(DependencySpecifier::Registry("*".to_string())),
        };
    }

    // file: prefix or relative/absolute path
    if let Some(path) = spec.strip_prefix("file:") {
        return DependencySpecifier::Path(PathBuf::from(path));
    }
    if spec.starts_with("./") || spec.starts_with("../") || spec.starts_with('/') {
        return DependencySpecifier::Path(PathBuf::from(spec));
    }

    // git+ prefix
    if spec.starts_with("git+") || spec.starts_with("git://") {
        let url = spec.strip_prefix("git+").unwrap_or(spec);
        let (url, commitish) = split_commitish(url);
        return DependencySpecifier::Git(GitSpec {
            url: url.to_string(),
            commitish,
        });
    }

    // GitHub/GitLab/Bitbucket shorthand
    if spec.starts_with("github:") || spec.starts_with("gitlab:") || spec.starts_with("bitbucket:")
    {
        let (host_prefix, rest) = spec.split_once(':').unwrap();
        let (repo_path, commitish) = split_commitish(rest);
        let host = match host_prefix {
            "github" => "github.com",
            "gitlab" => "gitlab.com",
            "bitbucket" => "bitbucket.org",
            _ => unreachable!(),
        };
        return DependencySpecifier::Git(GitSpec {
            url: format!("https://{}/{}.git", host, repo_path),
            commitish,
        });
    }

    // user/repo shorthand (GitHub)
    if looks_like_github_shorthand(spec) {
        let (repo_path, commitish) = split_commitish(spec);
        return DependencySpecifier::Git(GitSpec {
            url: format!("https://github.com/{}.git", repo_path),
            commitish,
        });
    }

    // Tarball URL
    if spec.starts_with("https://") || spec.starts_with("http://") {
        // Could be a tarball URL
        if spec.ends_with(".tgz") || spec.ends_with(".tar.gz") || spec.contains("/tarball/") {
            return DependencySpecifier::Url(spec.to_string());
        }
        // Could also be a git URL
        if spec.ends_with(".git") || spec.contains("github.com") || spec.contains("gitlab.com") {
            let (url, commitish) = split_commitish(spec);
            return DependencySpecifier::Git(GitSpec {
                url: url.to_string(),
                commitish,
            });
        }
        // Default to URL (tarball)
        return DependencySpecifier::Url(spec.to_string());
    }

    // Everything else is a registry specifier (semver range, tag, etc.)
    DependencySpecifier::Registry(spec.to_string())
}

/// Check if a spec looks like a GitHub shorthand (user/repo)
fn looks_like_github_shorthand(spec: &str) -> bool {
    // Must contain exactly one "/" and not start with "@"
    // Must not look like a semver range
    if spec.starts_with('@') || spec.contains("://") {
        return false;
    }
    let slash_count = spec.chars().filter(|c| *c == '/').count();
    if slash_count != 1 {
        return false;
    }
    // Should not start with special semver characters
    let first = spec.chars().next().unwrap_or(' ');
    !matches!(first, '^' | '~' | '>' | '<' | '=' | '0'..='9' | '*')
}

/// Split "url#commitish" into (url, Some(commitish)) or (url, None)
fn split_commitish(s: &str) -> (&str, Option<String>) {
    if let Some(pos) = s.rfind('#') {
        (&s[..pos], Some(s[pos + 1..].to_string()))
    } else {
        (s, None)
    }
}

/// Split "pkg@version" for alias parsing, handling scoped packages
fn split_alias(s: &str) -> Option<(&str, &str)> {
    if let Some(rest) = s.strip_prefix('@') {
        // Scoped: @scope/pkg@version
        if let Some(slash_pos) = rest.find('/') {
            let after_scope = &rest[slash_pos + 1..];
            if let Some(at_pos) = after_scope.find('@') {
                let name_end = 1 + slash_pos + 1 + at_pos; // account for '@' prefix
                return Some((&s[..name_end], &s[name_end + 1..]));
            }
        }
        None
    } else {
        // Unscoped: pkg@version
        s.find('@').map(|pos| (&s[..pos], &s[pos + 1..]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_specifiers() {
        assert!(matches!(parse("^1.2.3"), DependencySpecifier::Registry(_)));
        assert!(matches!(parse("~1.0.0"), DependencySpecifier::Registry(_)));
        assert!(matches!(parse(">=2.0.0"), DependencySpecifier::Registry(_)));
        assert!(matches!(parse("latest"), DependencySpecifier::Registry(_)));
        assert!(matches!(parse("*"), DependencySpecifier::Registry(_)));
    }

    #[test]
    fn test_git_specifiers() {
        match parse("github:user/repo") {
            DependencySpecifier::Git(spec) => {
                assert_eq!(spec.url, "https://github.com/user/repo.git");
                assert!(spec.commitish.is_none());
            }
            _ => panic!("Expected Git specifier"),
        }

        match parse("github:user/repo#v1.0.0") {
            DependencySpecifier::Git(spec) => {
                assert_eq!(spec.url, "https://github.com/user/repo.git");
                assert_eq!(spec.commitish.as_deref(), Some("v1.0.0"));
            }
            _ => panic!("Expected Git specifier"),
        }

        match parse("git+https://example.com/repo.git#main") {
            DependencySpecifier::Git(spec) => {
                assert_eq!(spec.url, "https://example.com/repo.git");
                assert_eq!(spec.commitish.as_deref(), Some("main"));
            }
            _ => panic!("Expected Git specifier"),
        }
    }

    #[test]
    fn test_path_specifiers() {
        assert!(matches!(
            parse("file:../local-pkg"),
            DependencySpecifier::Path(_)
        ));
        assert!(matches!(parse("./local-pkg"), DependencySpecifier::Path(_)));
        assert!(matches!(
            parse("../local-pkg"),
            DependencySpecifier::Path(_)
        ));
    }

    #[test]
    fn test_url_specifiers() {
        assert!(matches!(
            parse("https://example.com/pkg.tgz"),
            DependencySpecifier::Url(_)
        ));
    }

    #[test]
    fn test_alias_specifiers() {
        match parse("npm:other-pkg@^1.0") {
            DependencySpecifier::Alias { package, spec } => {
                assert_eq!(package, "other-pkg");
                assert!(matches!(*spec, DependencySpecifier::Registry(_)));
            }
            _ => panic!("Expected Alias specifier"),
        }
    }

    #[test]
    fn test_github_shorthand() {
        match parse("user/repo") {
            DependencySpecifier::Git(spec) => {
                assert_eq!(spec.url, "https://github.com/user/repo.git");
            }
            _ => panic!("Expected Git specifier"),
        }
    }
}
