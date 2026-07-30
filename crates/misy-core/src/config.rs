//! Local persistence for Misy-owned files: the versioned TOML configuration and the opaque
//! per-provider credential store. Both stores reject files with an unknown schema revision and
//! write atomically so a crash cannot leave a half-written file; credentials stay uninterpreted
//! JSON because their shape belongs to provider plugins, not to the core.
use crate::domain::{ModelRef, ProviderId};
use atomicwrites::{AllowOverwrite, AtomicFile};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
};

/// Paths owned by Misy.
///
/// [`Self::from_root`] keeps filesystem tests independent from the user home directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MisyPaths {
    root: PathBuf,
}

impl MisyPaths {
    /// Resolves Misy's data directory below the current user's home directory.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::HomeDirectoryUnavailable`] when neither supported home-directory
    /// environment variable is set.
    pub fn from_home() -> Result<Self, ConfigError> {
        let home = env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .ok_or(ConfigError::HomeDirectoryUnavailable)?;
        Ok(Self::from_root(PathBuf::from(home).join(".misy")))
    }

    /// Creates paths below an explicit root selected by a client or command-line invocation.
    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Returns the directory containing all Misy-owned files.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the versioned TOML configuration path.
    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    /// Returns the opaque provider-credential store path.
    pub fn credentials_file(&self) -> PathBuf {
        self.root.join("credentials.json")
    }

    /// Returns the persisted provider model-catalog cache path.
    pub fn models_file(&self) -> PathBuf {
        self.root.join("models.json")
    }

    /// Returns the directory containing append-only conversation sessions.
    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    /// Returns the directory containing user-installed provider packages.
    pub fn provider_plugins_dir(&self) -> PathBuf {
        self.root.join("plugins").join("providers")
    }

    /// Returns the directory containing global child-agent role definitions.
    pub fn agents_dir(&self) -> PathBuf {
        self.root.join("agents")
    }
}

/// Core-owned child-agent limits fixed when a root conversation starts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentConfig {
    /// Root-inclusive number of concurrently live agent threads.
    #[serde(default = "default_agent_threads")]
    pub max_concurrent_threads_per_session: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_concurrent_threads_per_session: default_agent_threads(),
        }
    }
}

/// Automatic and manual context-compaction settings.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CompactionConfig {
    /// Whether known-window sessions compact before crossing their safe threshold.
    #[serde(default = "default_true")]
    pub auto: bool,
    /// Fraction of the context window that triggers maintenance.
    #[serde(default = "default_trigger_ratio")]
    pub trigger_ratio: f64,
    /// Explicit reserved tokens, or a model-window-derived reserve when absent.
    #[serde(default)]
    pub reserve_tokens: Option<u32>,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            auto: true,
            trigger_ratio: default_trigger_ratio(),
            reserve_tokens: None,
        }
    }
}

const fn default_agent_threads() -> usize {
    4
}

const fn default_true() -> bool {
    true
}

const fn default_trigger_ratio() -> f64 {
    0.85
}

/// The on-disk configuration format. Versions are explicit so incompatible formats are rejected.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Config {
    /// On-disk schema revision used to reject incompatible files.
    pub version: u32,
    /// Model selected for direct interaction, if one has been saved.
    pub default_model: Option<ModelRef>,
    /// Provider-owned reasoning level paired with [`Self::default_model`].
    #[serde(default)]
    pub default_thinking: Option<String>,
    /// Frontend-owned named keybinding overrides retained across core updates.
    #[serde(default)]
    pub keybindings: BTreeMap<String, Vec<String>>,
    /// Root-inclusive agent-tree admission settings.
    #[serde(default)]
    pub agents: AgentConfig,
    /// Context compaction settings.
    #[serde(default)]
    pub compaction: CompactionConfig,
}

impl Config {
    /// Current configuration schema revision.
    pub const VERSION: u32 = 2;
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: Self::VERSION,
            default_model: None,
            default_thinking: None,
            keybindings: BTreeMap::new(),
            agents: AgentConfig::default(),
            compaction: CompactionConfig::default(),
        }
    }
}

// The established public name describes errors from the `config` contract.
#[allow(clippy::module_name_repetitions)]
#[derive(Debug)]
/// Errors while locating, reading, parsing, or serializing configuration.
pub enum ConfigError {
    /// No supported home-directory environment variable was available.
    HomeDirectoryUnavailable,
    /// A filesystem operation failed.
    Io(std::io::Error),
    /// TOML configuration could not be parsed.
    Parse(toml::de::Error),
    /// Configuration could not be serialized as TOML.
    Serialize(toml::ser::Error),
    /// The file uses a schema revision this build does not support.
    UnsupportedVersion(u32),
    /// A current-version setting is outside its accepted range.
    InvalidValue(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HomeDirectoryUnavailable => {
                formatter.write_str("could not determine the home directory")
            }
            Self::Io(error) => write!(formatter, "filesystem error: {error}"),
            Self::Parse(error) => write!(formatter, "invalid config file: {error}"),
            Self::Serialize(error) => write!(formatter, "could not serialize config: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported config version {version}")
            }
            Self::InvalidValue(message) => write!(formatter, "invalid configuration: {message}"),
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Parse(error) => Some(error),
            Self::Serialize(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ConfigError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Loads and saves versioned configuration under one `MisyPaths` root.
// The established public name identifies the configuration persistence contract.
#[allow(clippy::module_name_repetitions)]
#[derive(Clone, Debug)]
pub struct ConfigStore {
    paths: MisyPaths,
}

// The established public name describes errors from the credential-store contract.
#[allow(clippy::module_name_repetitions)]
#[derive(Debug)]
/// Errors while reading, validating, or writing opaque provider credentials.
pub enum CredentialError {
    /// A filesystem operation failed.
    Io(std::io::Error),
    /// The JSON credential document could not be parsed.
    Parse(serde_json::Error),
    /// Credentials could not be serialized as JSON.
    Serialize(serde_json::Error),
    /// The file uses a schema revision this build does not support.
    UnsupportedVersion(u32),
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "filesystem error: {error}"),
            Self::Parse(error) => write!(formatter, "invalid credentials file: {error}"),
            Self::Serialize(error) => write!(formatter, "could not serialize credentials: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported credentials version {version}")
            }
        }
    }
}

impl Error for CredentialError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Parse(error) => Some(error),
            Self::Serialize(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for CredentialError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct CredentialsFile {
    version: u32,
    providers: BTreeMap<ProviderId, serde_json::Value>,
}

const CREDENTIALS_VERSION: u32 = 1;

impl Default for CredentialsFile {
    fn default() -> Self {
        Self {
            version: CREDENTIALS_VERSION,
            providers: BTreeMap::new(),
        }
    }
}

/// Persists opaque provider credentials without interpreting provider-specific fields.
#[derive(Clone, Debug)]
pub struct CredentialStore {
    paths: MisyPaths,
}

impl CredentialStore {
    /// Creates a credential store rooted at `paths`.
    pub fn new(paths: MisyPaths) -> Self {
        Self { paths }
    }

    /// Loads one provider's opaque credential value, if it has been stored.
    ///
    /// # Errors
    ///
    /// Returns an error when the credential file cannot be read, parsed, or validated.
    pub fn load(
        &self,
        provider: &crate::domain::ProviderId,
    ) -> Result<Option<serde_json::Value>, CredentialError> {
        let document = self.read_file()?;
        Ok(document.providers.get(provider).cloned())
    }

    /// Atomically saves one provider's opaque credential value with private permissions.
    ///
    /// # Errors
    ///
    /// Returns an error when existing credentials cannot be read or the updated file cannot be
    /// serialized or written.
    pub fn save(
        &self,
        provider: &crate::domain::ProviderId,
        credentials: serde_json::Value,
    ) -> Result<(), CredentialError> {
        let mut document = self.read_file()?;
        document.providers.insert(provider.clone(), credentials);
        let contents = serde_json::to_vec_pretty(&document).map_err(CredentialError::Serialize)?;
        write_atomic_private(&self.paths.credentials_file(), &contents).map_err(CredentialError::Io)
    }

    /// Removes one provider's opaque credential record after a successful logout.
    ///
    /// # Errors
    ///
    /// Returns an error when the credential document cannot be read or the updated document
    /// cannot be serialized or written.
    pub fn remove(&self, provider: &crate::domain::ProviderId) -> Result<(), CredentialError> {
        let mut document = self.read_file()?;
        document.providers.remove(provider);
        let contents = serde_json::to_vec_pretty(&document).map_err(CredentialError::Serialize)?;
        write_atomic_private(&self.paths.credentials_file(), &contents).map_err(CredentialError::Io)
    }

    fn read_file(&self) -> Result<CredentialsFile, CredentialError> {
        let path = self.paths.credentials_file();
        let contents = match fs::read(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CredentialsFile::default());
            }
            Err(error) => return Err(error.into()),
        };
        let document: CredentialsFile =
            serde_json::from_slice(&contents).map_err(CredentialError::Parse)?;
        if document.version != CREDENTIALS_VERSION {
            return Err(CredentialError::UnsupportedVersion(document.version));
        }
        Ok(document)
    }
}

impl ConfigStore {
    /// Creates a configuration store rooted at `paths`.
    pub fn new(paths: MisyPaths) -> Self {
        Self { paths }
    }

    /// Loads configuration, returning the current defaults if no file exists.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read, parsed, or validated.
    pub fn load(&self) -> Result<Config, ConfigError> {
        let path = self.paths.config_file();
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Config::default());
            }
            Err(error) => return Err(error.into()),
        };
        let mut config: Config = toml::from_str(&contents).map_err(ConfigError::Parse)?;
        if config.version == 1 {
            config.version = Config::VERSION;
            validate_config(&config)?;
            self.save(&config)?;
        } else if config.version != Config::VERSION {
            return Err(ConfigError::UnsupportedVersion(config.version));
        }
        validate_config(&config)?;
        Ok(config)
    }

    /// Atomically saves a current-version configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when `config` has an unsupported version or cannot be serialized or
    /// written.
    pub fn save(&self, config: &Config) -> Result<(), ConfigError> {
        if config.version != Config::VERSION {
            return Err(ConfigError::UnsupportedVersion(config.version));
        }
        validate_config(config)?;
        let contents = toml::to_string_pretty(config).map_err(ConfigError::Serialize)?;
        write_atomic(&self.paths.config_file(), contents.as_bytes()).map_err(ConfigError::Io)
    }
}

fn validate_config(config: &Config) -> Result<(), ConfigError> {
    if !(1..=64).contains(&config.agents.max_concurrent_threads_per_session) {
        return Err(ConfigError::InvalidValue(
            "agents.max_concurrent_threads_per_session must be in 1..=64".to_owned(),
        ));
    }
    if !(0.50..=0.95).contains(&config.compaction.trigger_ratio)
        || !config.compaction.trigger_ratio.is_finite()
    {
        return Err(ConfigError::InvalidValue(
            "compaction.trigger_ratio must be in 0.50..=0.95".to_owned(),
        ));
    }
    if config
        .compaction
        .reserve_tokens
        .is_some_and(|tokens| !(1_000..=1_000_000).contains(&tokens))
    {
        return Err(ConfigError::InvalidValue(
            "compaction.reserve_tokens must be in 1000..=1000000".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    write_atomic_with_mode(path, contents, false)
}

fn write_atomic_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    write_atomic_with_mode(path, contents, true)
}

fn write_atomic_with_mode(path: &Path, contents: &[u8], private: bool) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path has no parent directory",
        )
    })?;
    fs::create_dir_all(parent)?;
    set_private_dir_permissions(parent)?;
    AtomicFile::new(path, AllowOverwrite)
        .write(|temporary_file| {
            if private {
                set_private_permissions(temporary_file)?;
            }
            temporary_file.write_all(contents)
        })
        .map_err(Into::into)
}

#[cfg(unix)]
fn set_private_permissions(file: &fs::File) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private_permissions(_file: &fs::File) -> std::io::Result<()> {
    Ok(())
}

// The data directory holds provider and model metadata that other local users must not read,
// so it is tightened to the `~/.ssh` convention. Runs on every write, not only on creation,
// to remediate directories left at umask defaults by older versions.
#[cfg(unix)]
fn set_private_dir_permissions(directory: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if fs::metadata(directory)?.permissions().mode() & 0o777 != 0o700 {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_directory: &Path) -> std::io::Result<()> {
    Ok(())
}
