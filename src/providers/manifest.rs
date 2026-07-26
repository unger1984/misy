use super::protocol::PROVIDER_PROTOCOL_VERSION;
use crate::ProviderId;
use semver::Version;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

/// Metadata and launch information declared by one self-contained provider package.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ProviderManifest {
    /// Stable provider identifier used by the core and credential store.
    pub id: ProviderId,
    /// The provider name displayed to users.
    pub display_name: String,
    /// Provider package semantic version.
    pub version: Version,
    /// Package kind; provider packages must use `provider`.
    pub kind: String,
    /// JSON-RPC protocol revision required by this package.
    pub protocol_version: u32,
    /// User-facing summary of the provider and authentication method.
    pub description: String,
    /// Package author.
    pub author: String,
    /// Package homepage URL.
    pub homepage: String,
    /// Source repository URL.
    pub repository: String,
    /// Package license identifier or text reference.
    pub license: String,
    /// Executable used to launch the package from its root directory.
    pub command: String,
    /// Arguments passed to [`Self::command`].
    pub args: Vec<String>,
    /// Authentication mechanisms this provider supports.
    pub auth_methods: Vec<ProviderAuthMethod>,
}

/// One authentication mechanism a provider package can offer to a client.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ProviderAuthMethod {
    /// Stable provider-local identifier persisted with credentials.
    pub id: String,
    /// User-facing name for this method.
    pub display_name: String,
}

/// A discovered package and the root that owns its relative launch command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderPackage {
    root: PathBuf,
    manifest: ProviderManifest,
}

impl ProviderPackage {
    /// Returns the package root used as the provider process working directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the validated package manifest.
    pub fn manifest(&self) -> &ProviderManifest {
        &self.manifest
    }
}

/// The set of providers available without starting any subprocesses.
#[derive(Clone, Debug, Default)]
pub struct ProviderCatalog {
    packages: BTreeMap<String, ProviderPackage>,
}

impl ProviderCatalog {
    /// Finds bundled and installed packages. A provider ID must be globally unique.
    ///
    /// # Errors
    ///
    /// Returns an error for filesystem failures, invalid manifests, duplicate IDs, or unsupported
    /// protocol versions.
    pub fn discover(
        bundled_root: &Path,
        installed_root: &Path,
    ) -> Result<Self, ProviderDiscoveryError> {
        let mut packages = BTreeMap::new();
        for root in [bundled_root, installed_root] {
            for package in discover_root(root)? {
                let id = package.manifest.id.as_str().to_owned();
                if packages.insert(id.clone(), package).is_some() {
                    return Err(ProviderDiscoveryError::DuplicateProvider(id));
                }
            }
        }
        Ok(Self { packages })
    }

    /// Returns a package by stable provider ID.
    pub fn get(&self, id: &str) -> Option<&ProviderPackage> {
        self.packages.get(id)
    }

    /// Returns the number of discovered packages.
    pub fn len(&self) -> usize {
        self.packages.len()
    }

    /// Reports whether discovery found no packages.
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }

    /// Iterates packages in stable provider-ID order.
    pub fn packages(&self) -> impl Iterator<Item = &ProviderPackage> {
        self.packages.values()
    }
}

fn discover_root(root: &Path) -> Result<Vec<ProviderPackage>, ProviderDiscoveryError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(ProviderDiscoveryError::Io(error)),
    };
    let mut packages = Vec::new();
    for entry in entries {
        let entry = entry.map_err(ProviderDiscoveryError::Io)?;
        let file_type = entry.file_type().map_err(ProviderDiscoveryError::Io)?;
        if !file_type.is_dir() {
            continue;
        }
        let root = entry.path();
        let manifest_path = root.join("misy-plugin.json");
        if !manifest_path.is_file() {
            continue;
        }
        let contents = fs::read_to_string(&manifest_path).map_err(ProviderDiscoveryError::Io)?;
        let manifest: ProviderManifest = serde_json::from_str(&contents).map_err(|source| {
            ProviderDiscoveryError::InvalidManifest {
                path: manifest_path.clone(),
                source,
            }
        })?;
        validate_manifest(&manifest, &manifest_path)?;
        packages.push(ProviderPackage { root, manifest });
    }
    Ok(packages)
}

fn validate_manifest(
    manifest: &ProviderManifest,
    path: &Path,
) -> Result<(), ProviderDiscoveryError> {
    if manifest.id.as_str().is_empty()
        || manifest.display_name.trim().is_empty()
        || manifest.command.trim().is_empty()
    {
        return Err(ProviderDiscoveryError::InvalidManifestValue {
            path: path.to_owned(),
            message: "id, display_name, and command must not be empty".to_owned(),
        });
    }
    if manifest.kind != "provider" {
        return Err(ProviderDiscoveryError::InvalidManifestValue {
            path: path.to_owned(),
            message: "kind must be `provider`".to_owned(),
        });
    }
    if [
        &manifest.description,
        &manifest.author,
        &manifest.homepage,
        &manifest.repository,
        &manifest.license,
    ]
    .into_iter()
    .any(|value| value.trim().is_empty())
    {
        return Err(ProviderDiscoveryError::InvalidManifestValue {
            path: path.to_owned(),
            message: "description, author, homepage, repository, and license must not be empty"
                .to_owned(),
        });
    }
    if manifest.protocol_version != PROVIDER_PROTOCOL_VERSION {
        return Err(ProviderDiscoveryError::UnsupportedProtocol {
            id: manifest.id.as_str().to_owned(),
            found: manifest.protocol_version,
        });
    }
    if manifest.auth_methods.is_empty()
        || manifest
            .auth_methods
            .iter()
            .any(|method| method.id.trim().is_empty() || method.display_name.trim().is_empty())
        || manifest
            .auth_methods
            .iter()
            .map(|method| method.id.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != manifest.auth_methods.len()
    {
        return Err(ProviderDiscoveryError::InvalidManifestValue {
            path: path.to_owned(),
            message: "auth_methods must contain unique, non-empty IDs and display names".to_owned(),
        });
    }
    Ok(())
}

/// Errors reported while discovering and validating provider packages.
#[derive(Debug)]
pub enum ProviderDiscoveryError {
    /// A directory or manifest filesystem operation failed.
    Io(std::io::Error),
    /// A manifest was not valid JSON.
    InvalidManifest {
        /// Path to the invalid manifest.
        path: PathBuf,
        /// JSON parser error.
        source: serde_json::Error,
    },
    /// A syntactically valid manifest violated the package contract.
    InvalidManifestValue {
        /// Path to the invalid manifest.
        path: PathBuf,
        /// Description of the violated requirement.
        message: String,
    },
    /// More than one package declared the same provider ID.
    DuplicateProvider(String),
    /// A package requires a protocol revision this host does not implement.
    UnsupportedProtocol {
        /// Provider declaring the unsupported revision.
        id: String,
        /// Required protocol revision.
        found: u32,
    },
}

impl fmt::Display for ProviderDiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "provider package filesystem error: {error}"),
            Self::InvalidManifest { path, source } => {
                write!(
                    formatter,
                    "invalid provider manifest {}: {source}",
                    path.display()
                )
            }
            Self::InvalidManifestValue { path, message } => {
                write!(
                    formatter,
                    "invalid provider manifest {}: {message}",
                    path.display()
                )
            }
            Self::DuplicateProvider(id) => write!(formatter, "duplicate provider ID `{id}`"),
            Self::UnsupportedProtocol { id, found } => write!(
                formatter,
                "provider `{id}` requires unsupported protocol version {found}"
            ),
        }
    }
}

impl Error for ProviderDiscoveryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidManifest { source, .. } => Some(source),
            _ => None,
        }
    }
}
