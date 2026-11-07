//! GitHub connection through the official CLI. Credentials stay with `gh`
//! and the system keychain; KERN never reads or stores tokens.
use crate::platform::tools;
use anyhow::{Result, anyhow};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthState {
    CliMissing,
    SignedOut,
    SignedIn,
}

pub fn auth_state() -> AuthState {
    if tools::find("gh").is_none() {
        return AuthState::CliMissing;
    }
    let out = tools::command("gh")
        .args(["auth", "status", "--hostname", "github.com"])
        .output();
    match out {
        Ok(o) if o.status.success() => AuthState::SignedIn,
        _ => AuthState::SignedOut,
    }
}

pub fn account_login() -> Option<String> {
    let out = tools::command("gh")
        .args(["api", "user", "--jq", ".login"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Shell command for the official browser flow; it runs in a terminal tab so
/// that the one-time code is visible and `gh` keeps full control of the flow.
pub const LOGIN_ARGS: &[&str] = &[
    "auth",
    "login",
    "--hostname",
    "github.com",
    "--web",
    "--git-protocol",
    "https",
];

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Repo {
    #[serde(rename = "nameWithOwner")]
    pub name_with_owner: String,
    #[serde(rename = "isPrivate", default)]
    pub is_private: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub url: String,
}

pub fn parse_repos(json: &[u8]) -> Result<Vec<Repo>> {
    Ok(serde_json::from_slice(json)?)
}

/// Repositories of the signed-in user and of their organizations.
pub fn list_repos() -> Result<Vec<Repo>> {
    let mut all = list_for(None)?;
    if let Ok(orgs) = tools::command("gh")
        .args(["api", "user/orgs", "--jq", ".[].login"])
        .output()
    {
        for org in String::from_utf8_lossy(&orgs.stdout).lines() {
            if let Ok(repos) = list_for(Some(org.trim())) {
                all.extend(repos);
            }
        }
    }
    all.dedup_by(|a, b| a.name_with_owner == b.name_with_owner);
    Ok(all)
}

fn list_for(owner: Option<&str>) -> Result<Vec<Repo>> {
    let mut cmd = tools::command("gh");
    cmd.args(["repo", "list"]);
    if let Some(o) = owner {
        cmd.arg(o);
    }
    cmd.args([
        "--limit",
        "200",
        "--json",
        "nameWithOwner,isPrivate,description,url",
    ]);
    let out = cmd.output().map_err(|e| anyhow!("cannot run gh: {e}"))?;
    if !out.status.success() {
        return Err(anyhow!("{}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    parse_repos(&out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repo_list() {
        let r = parse_repos(br#"[{"nameWithOwner":"o/r","isPrivate":true,"description":null,"url":"https://github.com/o/r"}]"#).unwrap();
        assert_eq!(r[0].name_with_owner, "o/r");
        assert!(r[0].is_private);
        assert!(parse_repos(b"nope").is_err());
    }
}
