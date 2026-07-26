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
    pub id: ProviderId,
    pub version: Version,
    pub kind: String,
    pub protocol_version: u32,
    pub description: String,
    pub author: String,
    pub homepage: String,
    pub repository: String,
    pub license: String,
    pub command: String,
    pub args: Vec<String>,
}

/// A discovered package and the root that owns its relative launch command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderPackage {
    root: PathBuf,
    manifest: ProviderManifest,
}

impl ProviderPackage {
    pub fn root(&self) -> &Path {
        &self.root
    }

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

    pub fn get(&self, id: &str) -> Option<&ProviderPackage> {
        self.packages.get(id)
    }

    pub fn len(&self) -> usize {
        self.packages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }

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
    if manifest.id.as_str().is_empty() || manifest.command.trim().is_empty() {
        return Err(ProviderDiscoveryError::InvalidManifestValue {
            path: path.to_owned(),
            message: "id and command must not be empty".to_owned(),
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
    Ok(())
}

#[derive(Debug)]
pub enum ProviderDiscoveryError {
    Io(std::io::Error),
    InvalidManifest {
        path: PathBuf,
        source: serde_json::Error,
    },
    InvalidManifestValue {
        path: PathBuf,
        message: String,
    },
    DuplicateProvider(String),
    UnsupportedProtocol {
        id: String,
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
