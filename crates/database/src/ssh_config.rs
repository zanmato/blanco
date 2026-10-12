//! Reading `~/.ssh/config` the way OpenSSH does for the options a tunnel
//! needs: `HostName`, `User`, `Port`, `IdentityFile` and `ProxyJump`, with
//! `Host` patterns, `Include` files and first-value-wins semantics.

use std::path::{Path, PathBuf};

/// How deep `Include` may nest, as in OpenSSH.
const MAX_INCLUDE_DEPTH: usize = 16;

/// The options that apply to one host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolvedHost {
    pub host_name: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_files: Vec<PathBuf>,
    /// The `ProxyJump` value as written: `none`, or comma-separated hops.
    pub proxy_jump: Option<String>,
}

#[derive(Clone, Debug)]
struct Block {
    /// `Host` patterns; `None` for a `Match` block, which is never applied.
    patterns: Option<Vec<String>>,
    options: Vec<(String, String)>,
}

/// A parsed ssh config, its includes inlined in order.
#[derive(Clone, Debug, Default)]
pub struct SshConfig {
    blocks: Vec<Block>,
}

impl SshConfig {
    /// The user's `~/.ssh/config`, empty when there is none.
    pub fn load_user() -> Self {
        let Some(ssh_dir) = dirs::home_dir().map(|home| home.join(".ssh")) else {
            return Self::default();
        };
        match std::fs::read_to_string(ssh_dir.join("config")) {
            Ok(text) => Self::parse(&text, &ssh_dir),
            Err(_) => Self::default(),
        }
    }

    /// Parse config text; relative `Include` paths resolve against
    /// `include_dir` (`~/.ssh` for the user's config).
    pub fn parse(text: &str, include_dir: &Path) -> Self {
        let mut config = Self::default();
        config.parse_into(text, include_dir, vec!["*".to_string()], 0);
        config
    }

    fn parse_into(&mut self, text: &str, include_dir: &Path, patterns: Vec<String>, depth: usize) {
        // Options before the first `Host` line apply under the enclosing
        // condition: every host at the top level, the including block's
        // patterns inside an `Include`.
        let mut current = Block {
            patterns: Some(patterns),
            options: Vec::new(),
        };
        for line in text.lines() {
            let Some((keyword, value)) = split_line(line) else {
                continue;
            };
            match keyword.as_str() {
                "host" => {
                    self.blocks.push(std::mem::replace(
                        &mut current,
                        Block {
                            patterns: Some(split_values(&value)),
                            options: Vec::new(),
                        },
                    ));
                }
                "match" => {
                    self.blocks.push(std::mem::replace(
                        &mut current,
                        Block {
                            patterns: None,
                            options: Vec::new(),
                        },
                    ));
                }
                "include" if depth < MAX_INCLUDE_DEPTH => {
                    // Included options take effect where the Include line is.
                    let enclosing = current.patterns.clone();
                    self.blocks.push(std::mem::replace(
                        &mut current,
                        Block {
                            patterns: enclosing.clone(),
                            options: Vec::new(),
                        },
                    ));
                    let Some(enclosing) = enclosing else {
                        continue;
                    };
                    for pattern in split_values(&value) {
                        for path in expand_include(&pattern, include_dir) {
                            if let Ok(text) = std::fs::read_to_string(&path) {
                                self.parse_into(&text, include_dir, enclosing.clone(), depth + 1);
                            }
                        }
                    }
                }
                _ => current.options.push((keyword, value)),
            }
        }
        self.blocks.push(current);
    }

    /// The options for `alias`, the name typed as the SSH host.
    pub fn resolve(&self, alias: &str) -> ResolvedHost {
        let mut resolved = ResolvedHost::default();
        for block in &self.blocks {
            let Some(patterns) = &block.patterns else {
                continue;
            };
            if !host_matches(patterns, alias) {
                continue;
            }
            for (keyword, value) in &block.options {
                let value = unquote(value);
                match keyword.as_str() {
                    "hostname" if resolved.host_name.is_none() => {
                        resolved.host_name = Some(value.replace("%h", alias));
                    }
                    "user" if resolved.user.is_none() => resolved.user = Some(value),
                    "port" if resolved.port.is_none() => resolved.port = value.parse().ok(),
                    "identityfile" => resolved.identity_files.push(expand_tilde(&value)),
                    "proxyjump" if resolved.proxy_jump.is_none() => {
                        resolved.proxy_jump = Some(value)
                    }
                    _ => {}
                }
            }
        }
        resolved
    }
}

/// One hop of a `ProxyJump` list: `[user@]host[:port]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JumpHost {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

/// Parse a `ProxyJump` value. `none` and an empty value mean no jumps.
pub fn parse_jump_hosts(value: &str) -> Result<Vec<JumpHost>, String> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(str::trim)
        .map(|hop| {
            let hop = hop.strip_prefix("ssh://").unwrap_or(hop);
            let (user, rest) = match hop.rsplit_once('@') {
                Some((user, rest)) => (Some(user.to_string()), rest),
                None => (None, hop),
            };
            let (host, port) = if let Some(bracketed) = rest.strip_prefix('[') {
                // [IPv6]:port
                let (host, after) = bracketed
                    .split_once(']')
                    .ok_or_else(|| format!("Invalid jump host \"{hop}\""))?;
                (host, after.strip_prefix(':'))
            } else {
                match rest.rsplit_once(':') {
                    Some((host, port)) => (host, Some(port)),
                    None => (rest, None),
                }
            };
            if host.is_empty() {
                return Err(format!("Invalid jump host \"{hop}\""));
            }
            let port = port
                .map(|port| {
                    port.parse::<u16>()
                        .map_err(|_| format!("Invalid port in jump host \"{hop}\""))
                })
                .transpose()?;
            Ok(JumpHost {
                user: user.filter(|user| !user.is_empty()),
                host: host.to_string(),
                port,
            })
        })
        .collect()
}

/// `keyword value` or `keyword=value`, comments and blank lines skipped. The
/// keyword is lowercased.
fn split_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let split = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let keyword = line[..split].to_ascii_lowercase();
    let value = line[split..]
        .trim_start_matches(|c: char| c.is_whitespace())
        .trim_start_matches('=')
        .trim();
    Some((keyword, value.to_string()))
}

/// Whitespace separated values, with double quotes grouping.
fn split_values(value: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in value.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    values.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        values.push(current);
    }
    values
}

fn unquote(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
        .to_string()
}

fn expand_tilde(path: &str) -> PathBuf {
    let home = dirs::home_dir();
    let path = match &home {
        Some(home) => path.replace("%d", &home.to_string_lossy()),
        None => path.to_string(),
    };
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

/// The files an `Include` pattern names: relative paths are under
/// `include_dir`, and `*`/`?` in the file name match within its directory.
fn expand_include(pattern: &str, include_dir: &Path) -> Vec<PathBuf> {
    let path = expand_tilde(pattern);
    let path = if path.is_absolute() {
        path
    } else {
        include_dir.join(path)
    };
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !name.contains(['*', '?']) {
        return vec![path];
    }
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let mut matches: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|candidate| {
            candidate
                .file_name()
                .is_some_and(|file| wildcard_match(&name, &file.to_string_lossy()))
        })
        .collect();
    matches.sort();
    matches
}

/// Whether `host` matches a `Host` line: some positive pattern matches and
/// no `!pattern` does.
fn host_matches(patterns: &[String], host: &str) -> bool {
    let mut matched = false;
    for pattern in patterns {
        match pattern.strip_prefix('!') {
            Some(negated) if wildcard_match(negated, host) => return false,
            Some(_) => {}
            None => matched |= wildcard_match(pattern, host),
        }
    }
    matched
}

/// `*` and `?` wildcards, ignoring ASCII case as OpenSSH does for host names.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.to_ascii_lowercase().chars().collect();
    let text: Vec<char> = text.to_ascii_lowercase().chars().collect();
    let (mut p, mut t) = (0, 0);
    let (mut star, mut star_text) = (None, 0);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            star_text = t;
            p += 1;
        } else if let Some(star) = star {
            p = star + 1;
            star_text += 1;
            t = star_text;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_first_values_and_accumulates_identity_files() {
        let config = SshConfig::parse(
            "\
# global
IdentityFile ~/.ssh/global_key

Host db-prod db-stage
    HostName %h.internal.example.com
    User deploy
    Port 2222
    IdentityFile ~/.ssh/deploy_ed25519
    ProxyJump bastion

Host db-*
    User other
    ProxyJump none

Host bastion
    HostName=bastion.example.com
    User \"jump user\"
",
            Path::new("/nowhere"),
        );
        let home = dirs::home_dir().unwrap();
        let prod = config.resolve("db-prod");
        assert_eq!(
            prod,
            ResolvedHost {
                host_name: Some("db-prod.internal.example.com".into()),
                user: Some("deploy".into()),
                port: Some(2222),
                identity_files: vec![
                    home.join(".ssh/global_key"),
                    home.join(".ssh/deploy_ed25519")
                ],
                proxy_jump: Some("bastion".into()),
            }
        );
        let other = config.resolve("db-other");
        assert_eq!(other.user.as_deref(), Some("other"));
        assert_eq!(other.proxy_jump.as_deref(), Some("none"));
        assert_eq!(other.host_name, None);
        let bastion = config.resolve("BASTION");
        assert_eq!(bastion.host_name.as_deref(), Some("bastion.example.com"));
        assert_eq!(bastion.user.as_deref(), Some("jump user"));
    }

    #[test]
    fn negated_patterns_and_match_blocks() {
        let config = SshConfig::parse(
            "Host *.example.com !secret.example.com\n  User web\nMatch host *\n  User matched\nHost *\n  User fallback\n",
            Path::new("/nowhere"),
        );
        assert_eq!(config.resolve("a.example.com").user.as_deref(), Some("web"));
        assert_eq!(
            config.resolve("secret.example.com").user.as_deref(),
            Some("fallback")
        );
        assert_eq!(config.resolve("x.y").user.as_deref(), Some("fallback"));
    }

    #[test]
    fn includes_are_inlined_in_order_and_under_their_host() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("config.d")).unwrap();
        std::fs::write(
            dir.path().join("config.d/10-db"),
            "Host db\n  HostName db.from.include\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("config.d/20-db"),
            "Host db\n  HostName second\n  Port 2200\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("scoped"), "User scoped-user\n").unwrap();
        let config = SshConfig::parse(
            "Include config.d/*\nHost scoped\n  Include scoped\nHost *\n  User everyone\n",
            dir.path(),
        );
        let db = config.resolve("db");
        assert_eq!(db.host_name.as_deref(), Some("db.from.include"));
        assert_eq!(db.port, Some(2200));
        assert_eq!(db.user.as_deref(), Some("everyone"));
        assert_eq!(
            config.resolve("scoped").user.as_deref(),
            Some("scoped-user")
        );
    }

    #[test]
    fn include_loops_stop() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("loop"), "Include loop\nUser looped\n").unwrap();
        let config = SshConfig::parse("Include loop\n", dir.path());
        assert_eq!(config.resolve("x").user.as_deref(), Some("looped"));
    }

    #[test]
    fn jump_host_lists() {
        assert_eq!(parse_jump_hosts("none"), Ok(vec![]));
        assert_eq!(
            parse_jump_hosts("alice@bastion:2222, gw ,ssh://bob@[::1]:22"),
            Ok(vec![
                JumpHost {
                    user: Some("alice".into()),
                    host: "bastion".into(),
                    port: Some(2222)
                },
                JumpHost {
                    user: None,
                    host: "gw".into(),
                    port: None
                },
                JumpHost {
                    user: Some("bob".into()),
                    host: "::1".into(),
                    port: Some(22)
                },
            ])
        );
        assert!(parse_jump_hosts("host:port").is_err());
        assert!(parse_jump_hosts("user@").is_err());
    }

    #[test]
    fn wildcards() {
        assert!(wildcard_match("db-*", "DB-prod"));
        assert!(wildcard_match("h?st", "host"));
        assert!(wildcard_match("*", ""));
        assert!(!wildcard_match("db-*", "web"));
        assert!(wildcard_match("*.a.*", "x.a.y"));
    }
}
