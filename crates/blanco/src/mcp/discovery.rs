//! The agent workspace folder: a directory Blanco keeps up to date with
//! project-scoped MCP configuration pointing at the running instance, so any
//! agent launched with it as working directory (Claude Code, OpenCode, ...)
//! finds the server without manual setup. Tools without project-scoped config
//! (Codex) get a README with the snippet to paste.

use std::path::{Path, PathBuf};

use anyhow::Context as _;

pub(crate) const WORKSPACE_DIR_NAME: &str = "agent-workspace";

/// Files this module owns inside the workspace. Anything else in the folder is
/// the user's and is left alone.
const GENERATED_FILES: [&str; 6] = [
    "mcp.json",
    ".mcp.json",
    "opencode.json",
    "README.md",
    "AGENTS.md",
    "CLAUDE.md",
];

/// `~/.local/share/blanco/agent-workspace` (platform equivalent). Falls back to
/// the config dir, then the current directory, so a headless environment still
/// gets a deterministic location.
pub(crate) fn workspace_dir() -> PathBuf {
    dirs::data_dir()
        .or_else(dirs::config_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("blanco")
        .join(WORKSPACE_DIR_NAME)
}

pub(crate) fn server_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

/// The one-liner that registers this instance with Claude Code by hand, for
/// users who prefer that over the workspace folder.
pub(crate) fn claude_mcp_add_command(port: u16, token: &str) -> String {
    format!(
        "claude mcp add --transport http blanco {} --header \"Authorization: Bearer {token}\"",
        server_url(port)
    )
}

/// Write every generated file for a server listening on `port` with `token`.
/// The directory is created `0700` and each file `0600`, since the token grants
/// full access to the user's databases.
pub(crate) fn write_workspace_config(dir: &Path, port: u16, token: &str) -> anyhow::Result<()> {
    create_private_dir(dir)?;
    let url = server_url(port);
    let bearer = format!("Bearer {token}");

    let discovery = serde_json::json!({
        "url": url,
        "token": token,
        "pid": std::process::id(),
    });
    write_private_file(&dir.join("mcp.json"), &pretty(&discovery)?)?;

    let claude_config = serde_json::json!({
        "mcpServers": {
            "blanco": {
                "type": "http",
                "url": url,
                "headers": { "Authorization": bearer },
            }
        }
    });
    write_private_file(&dir.join(".mcp.json"), &pretty(&claude_config)?)?;

    let opencode_config = serde_json::json!({
        "$schema": "https://opencode.ai/config.json",
        "mcp": {
            "blanco": {
                "type": "remote",
                "url": url,
                "headers": { "Authorization": bearer },
            }
        }
    });
    write_private_file(&dir.join("opencode.json"), &pretty(&opencode_config)?)?;

    write_private_file(&dir.join("README.md"), &readme(port, token))?;
    write_private_file(&dir.join("AGENTS.md"), AGENT_INSTRUCTIONS)?;
    // Claude Code reads CLAUDE.md and supports `@file` imports, so the
    // instructions live in AGENTS.md once and every harness sees the same text.
    write_private_file(&dir.join("CLAUDE.md"), "@AGENTS.md\n")?;
    Ok(())
}

/// What an agent started in the workspace needs to know about where it is.
/// Read by Codex and OpenCode as `AGENTS.md` and by Claude Code through
/// `CLAUDE.md`. Everything tab-specific comes from the environment the pane's
/// shell was started with, since one folder serves every tab.
const AGENT_INSTRUCTIONS: &str = "# Working inside Blanco\n\n\
You are running in a terminal pane of Blanco, a desktop SQL editor. This folder is Blanco's\n\
agent workspace, not a code project: there is nothing to build or edit here. Your job is to help\n\
the user with their databases through the `blanco` MCP server, which is already configured for\n\
this directory.\n\n\
## The tab you belong to\n\n\
The pane you were started from belongs to one editor tab. Its identity is in the environment:\n\n\
- `BLANCO_TAB_ID`: pass it as `tab: {\"id\": <value>}` to `read_tab`, `write_tab` and `run_tab`, and\n\
  to connection tools when the user has switched to another tab meanwhile.\n\
- `BLANCO_CONNECTION_ID`, `BLANCO_CONNECTION_NAME`, `BLANCO_DATABASE`: the connection and database\n\
  that tab works against. Connection tools default to the active tab's connection when\n\
  `connection_id` is omitted; pass `BLANCO_CONNECTION_ID` to be explicit.\n\n\
## How to work\n\n\
- Start by reading the tab (`read_tab`) and, when the task needs schema knowledge, `list_tables`\n\
  or `describe_table` on that connection.\n\
- Write SQL the user should see into the tab with `write_tab` and run it with `run_tab`, so the\n\
  result lands in the app's results grid. Use `run_sql` for lookups the user does not need to see.\n\
- Statements that modify data may open a confirmation dialog in Blanco that the user has to\n\
  accept; PROD connections and DROP/TRUNCATE always do. Say so when you run one, and never retry a\n\
  declined statement.\n\
- Results are capped at 100 rows by default (`max_rows` up to 1000); narrow queries rather than\n\
  paging through large tables.\n";

/// Remove the generated files, keeping the folder (and anything the user put
/// in it). Missing files are not an error: a fresh install has none.
pub(crate) fn remove_workspace_config(dir: &Path) -> anyhow::Result<()> {
    for name in GENERATED_FILES {
        let path = dir.join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("removing {}", path.display()));
            }
        }
    }
    Ok(())
}

fn pretty(value: &serde_json::Value) -> anyhow::Result<String> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    Ok(text)
}

fn readme(port: u16, token: &str) -> String {
    let url = server_url(port);
    let claude_command = claude_mcp_add_command(port, token);
    format!(
        "# Blanco agent workspace\n\n\
This folder is managed by Blanco. Start a coding agent with this directory as its\n\
working directory and it will find the Blanco MCP server (tabs, connections, SQL,\n\
schema) at `{url}`.\n\n\
Files regenerated every time the MCP server starts (the token rotates):\n\n\
- `.mcp.json`: project-scoped config for Claude Code (`claude`).\n\
- `opencode.json`: project config for OpenCode (`opencode`).\n\
- `mcp.json`: plain `{{url, token, pid}}` for scripts.\n\
- `AGENTS.md` and `CLAUDE.md`: instructions telling the agent it is inside Blanco.\n\n\
The shell Blanco opens here also exports `BLANCO_MCP_URL`, `BLANCO_MCP_TOKEN` and, per tab,\n\
`BLANCO_TAB_ID`, `BLANCO_CONNECTION_ID`, `BLANCO_CONNECTION_NAME` and `BLANCO_DATABASE`.\n\n\
## Tools without project-scoped MCP config\n\n\
Claude Code, registered globally instead of via `.mcp.json`:\n\n\
```\n{claude_command}\n```\n\n\
Codex reads `~/.codex/config.toml`:\n\n\
```toml\n\
[mcp_servers.blanco]\n\
url = \"{url}\"\n\
http_headers = {{ Authorization = \"Bearer {token}\" }}\n\
```\n"
    )
}

fn create_private_dir(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("restricting permissions on {}", dir.display()))?;
    }
    Ok(())
}

fn write_private_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    use std::io::Write as _;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("writing {}", path.display()))?;
    // `mode` only applies when the file is created, so an existing file keeps
    // whatever permissions it had; tighten it explicitly.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("restricting permissions on {}", path.display()))?;
    }
    file.write_all(contents.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_removes_generated_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("workspace");
        write_workspace_config(&dir, 7821, "secret-token").expect("write config");

        let claude: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join(".mcp.json")).expect("read .mcp.json"),
        )
        .expect("parse .mcp.json");
        assert_eq!(claude["mcpServers"]["blanco"]["type"], "http");
        assert_eq!(
            claude["mcpServers"]["blanco"]["url"],
            "http://127.0.0.1:7821/mcp"
        );
        assert_eq!(
            claude["mcpServers"]["blanco"]["headers"]["Authorization"],
            "Bearer secret-token"
        );

        let opencode: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("opencode.json")).expect("read opencode.json"),
        )
        .expect("parse opencode.json");
        assert_eq!(opencode["mcp"]["blanco"]["type"], "remote");

        let discovery: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("mcp.json")).expect("read mcp.json"),
        )
        .expect("parse mcp.json");
        assert_eq!(discovery["token"], "secret-token");
        assert_eq!(
            std::fs::read_to_string(dir.join("CLAUDE.md")).expect("read CLAUDE.md"),
            "@AGENTS.md\n"
        );
        let agents = std::fs::read_to_string(dir.join("AGENTS.md")).expect("read AGENTS.md");
        assert!(agents.contains("BLANCO_TAB_ID"));
        assert!(!agents.contains("secret-token"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for name in GENERATED_FILES {
                let mode = std::fs::metadata(dir.join(name))
                    .expect("metadata")
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode, 0o600, "{name} should be private");
            }
            let dir_mode = std::fs::metadata(&dir)
                .expect("dir metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700);
        }

        // A user file in the folder survives a rewrite and a removal.
        std::fs::write(dir.join("notes.md"), "mine").expect("write user file");
        write_workspace_config(&dir, 7822, "rotated").expect("rewrite config");
        let rotated: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("mcp.json")).expect("read mcp.json"),
        )
        .expect("parse mcp.json");
        assert_eq!(rotated["token"], "rotated");

        remove_workspace_config(&dir).expect("remove config");
        for name in GENERATED_FILES {
            assert!(!dir.join(name).exists(), "{name} should be removed");
        }
        assert!(dir.join("notes.md").exists());
        remove_workspace_config(&dir).expect("removing twice is fine");
    }

    #[test]
    fn claude_command_carries_url_and_token() {
        let command = claude_mcp_add_command(7821, "abc");
        assert!(command.contains("http://127.0.0.1:7821/mcp"));
        assert!(command.contains("Bearer abc"));
    }
}
