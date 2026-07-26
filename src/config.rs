use crate::domain::ModelRef;
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

/// Paths owned by misy. `from_root` keeps filesystem tests independent from the user home directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MisyPaths {
    root: PathBuf,
}

impl MisyPaths {
    pub fn from_home() -> Result<Self, ConfigError> {
        let home = env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .ok_or(ConfigError::HomeDirectoryUnavailable)?;
        Ok(Self::from_root(PathBuf::from(home).join(".misy")))
    }

    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    pub fn credentials_file(&self) -> PathBuf {
        self.root.join("credentials.json")
    }

    pub fn provider_plugins_dir(&self) -> PathBuf {
        self.root.join("plugins").join("providers")
    }
}

/// The on-disk configuration format. Versions are explicit so incompatible formats are rejected.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Config {
    pub version: u32,
    pub default_model: Option<ModelRef>,
}

impl Config {
    pub const VERSION: u32 = 1;

    pub fn with_default_model(default_model: ModelRef) -> Self {
        Self {
            version: Self::VERSION,
            default_model: Some(default_model),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: Self::VERSION,
            default_model: None,
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    HomeDirectoryUnavailable,
    Io(std::io::Error),
    Parse(toml::de::Error),
    Serialize(toml::ser::Error),
    UnsupportedVersion(u32),
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
#[derive(Clone, Debug)]
pub struct ConfigStore {
    paths: MisyPaths,
}

#[derive(Debug)]
pub enum CredentialError {
    Io(std::io::Error),
    Parse(serde_json::Error),
    Serialize(serde_json::Error),
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
    providers: BTreeMap<String, serde_json::Value>,
}

impl Default for CredentialsFile {
    fn default() -> Self {
        Self {
            version: Config::VERSION,
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
    pub fn new(paths: MisyPaths) -> Self {
        Self { paths }
    }

    pub fn load(
        &self,
        provider: &crate::domain::ProviderId,
    ) -> Result<Option<serde_json::Value>, CredentialError> {
        let document = self.read_file()?;
        Ok(document.providers.get(provider.as_str()).cloned())
    }

    pub fn save(
        &self,
        provider: &crate::domain::ProviderId,
        credentials: serde_json::Value,
    ) -> Result<(), CredentialError> {
        let mut document = self.read_file()?;
        document
            .providers
            .insert(provider.as_str().to_owned(), credentials);
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
        if document.version != Config::VERSION {
            return Err(CredentialError::UnsupportedVersion(document.version));
        }
        Ok(document)
    }
}

impl ConfigStore {
    pub fn new(paths: MisyPaths) -> Self {
        Self { paths }
    }

    pub fn load(&self) -> Result<Config, ConfigError> {
        let path = self.paths.config_file();
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Config::default());
            }
            Err(error) => return Err(error.into()),
        };
        let config: Config = toml::from_str(&contents).map_err(ConfigError::Parse)?;
        if config.version != Config::VERSION {
            return Err(ConfigError::UnsupportedVersion(config.version));
        }
        Ok(config)
    }

    pub fn save(&self, config: &Config) -> Result<(), ConfigError> {
        if config.version != Config::VERSION {
            return Err(ConfigError::UnsupportedVersion(config.version));
        }
        let contents = toml::to_string_pretty(config).map_err(ConfigError::Serialize)?;
        write_atomic(&self.paths.config_file(), contents.as_bytes()).map_err(ConfigError::Io)
    }
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
