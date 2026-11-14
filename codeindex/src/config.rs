//! Where the server is and how to talk to it: the `url`, `token` and `repos_dir` settings
//! (types `text` and `secret`, manifest `api = 2`), typed in the hub's Settings › Apps.
//!
//! Order of precedence for each key: the setting, then the environment (`CODE_INDEX_URL`,
//! `CODE_INDEX_TOKEN`), then the default (only the URL has one).

use std::path::PathBuf;

use serde_json::Value;

pub const DEFAULT_URL: &str = "http://127.0.0.1:8765";
pub const ENV_URL: &str = "CODE_INDEX_URL";
pub const ENV_TOKEN: &str = "CODE_INDEX_TOKEN";

/// What the app needs to reach the server.
#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    /// Base URL, without a trailing slash.
    pub url: String,
    /// Bearer token (`cix_...`); empty when not configured.
    pub token: String,
    /// Folder with the server's clones (`<server project>/.repos`), to open results in the editor.
    pub repos_dir: Option<PathBuf>,
}

// The token must never reach a log: print everything else.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("url", &self.url)
            .field("token", &if self.token.is_empty() { "<none>" } else { "<hidden>" })
            .field("repos_dir", &self.repos_dir)
            .finish()
    }
}

impl Config {
    pub fn configured(&self) -> bool {
        !self.token.is_empty()
    }

    /// Reads the app's settings (the JSON object of `start` and `settings`) and falls back to
    /// `env` (a lookup function, so tests do not touch the real environment).
    pub fn load(settings: &Value, env: &dyn Fn(&str) -> Option<String>) -> Config {
        let text = |key: &str| settings[key].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        let from_env = |key: &str| env(key).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let url = text("url").or_else(|| from_env(ENV_URL)).unwrap_or_else(|| DEFAULT_URL.to_string());
        Config {
            url: url.trim_end_matches('/').to_string(),
            token: text("token").or_else(|| from_env(ENV_TOKEN)).unwrap_or_default(),
            repos_dir: text("repos_dir").map(PathBuf::from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn defaults_without_settings_or_env() {
        let c = Config::load(&Value::Null, &no_env);
        assert_eq!(c.url, DEFAULT_URL);
        assert!(!c.configured());
        assert_eq!(c.repos_dir, None);
    }

    #[test]
    fn environment_fills_what_the_settings_lack() {
        let env = |k: &str| match k {
            ENV_URL => Some("https://ci.example.com/".to_string()),
            ENV_TOKEN => Some(" cix_env ".to_string()),
            _ => None,
        };
        let c = Config::load(&json!({"url": "", "token": "  ", "repos_dir": ""}), &env);
        assert_eq!((c.url.as_str(), c.token.as_str()), ("https://ci.example.com", "cix_env"));
        assert!(c.configured());
        assert_eq!(c.repos_dir, None);
    }

    #[test]
    fn settings_win_over_environment() {
        let env = |_: &str| Some("from-env".to_string());
        let c = Config::load(
            &json!({"url": "http://10.0.0.5:9000/", "token": "cix_set", "repos_dir": "/srv/.repos"}),
            &env,
        );
        assert_eq!(c.url, "http://10.0.0.5:9000");
        assert_eq!(c.token, "cix_set");
        assert_eq!(c.repos_dir, Some(PathBuf::from("/srv/.repos")));
    }

    #[test]
    fn debug_never_prints_the_token() {
        let c = Config { url: "http://x".into(), token: "cix_secret".into(), repos_dir: None };
        assert!(!format!("{c:?}").contains("cix_secret"));
    }
}
