//! Safe operating-system browser handoff for provider authorization.

use std::process::Command;

/// Opens an authorization URL outside the terminal UI.
pub trait BrowserHandoff {
    /// Opens an already validated web URL.
    ///
    /// # Errors
    ///
    /// Returns an error when the operating system cannot accept the handoff.
    fn open(&mut self, url: &str) -> Result<(), String>;
}

/// Browser handoff using the host platform's standard URL opener.
#[derive(Default)]
pub struct SystemBrowser;

/// Operating-system family used to form a browser process invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserPlatform {
    /// macOS `open` command.
    MacOs,
    /// Windows URL protocol handler.
    Windows,
    /// Freedesktop-compatible `xdg-open` command.
    Unix,
}

/// Direct process invocation for an authorization URL.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserCommand {
    /// Executable invoked directly without a shell.
    pub program: String,
    /// Arguments passed verbatim to the executable.
    pub args: Vec<String>,
}

/// Builds a shell-free platform browser command.
///
/// # Errors
///
/// Returns an error when `url` is not a whitespace-free HTTP(S) URL.
pub fn browser_command(platform: BrowserPlatform, url: &str) -> Result<BrowserCommand, String> {
    validate_authorization_url(url)?;
    let (program, arguments): (&str, &[&str]) = match platform {
        BrowserPlatform::MacOs => ("open", &[url]),
        BrowserPlatform::Windows => ("rundll32", &["url.dll,FileProtocolHandler", url]),
        BrowserPlatform::Unix => ("xdg-open", &[url]),
    };
    Ok(BrowserCommand {
        program: program.to_owned(),
        args: arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect(),
    })
}

impl BrowserHandoff for SystemBrowser {
    fn open(&mut self, url: &str) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        let platform = BrowserPlatform::MacOs;
        #[cfg(target_os = "windows")]
        let platform = BrowserPlatform::Windows;
        #[cfg(all(unix, not(target_os = "macos")))]
        let platform = BrowserPlatform::Unix;
        let specification = browser_command(platform, url)?;
        Command::new(specification.program)
            .args(specification.args)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("could not open authorization URL: {error}"))
    }
}

/// Validates a provider-supplied authorization location before OS handoff.
///
/// # Errors
///
/// Returns an error for non-HTTP(S) locations or URLs containing whitespace/control characters.
pub fn validate_authorization_url(url: &str) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://"))
        || url.chars().any(char::is_control)
        || url.chars().any(char::is_whitespace)
    {
        return Err("authorization URL must be an http(s) URL without whitespace".to_owned());
    }
    Ok(())
}
