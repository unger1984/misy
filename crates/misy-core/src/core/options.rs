//! Construction paths that bind immutable frontend capabilities to the core.

use super::{CoreError, CoreOptions, MisyCore};
use crate::{
    MisyPaths,
    providers::{ProviderCatalog, ProviderDeadlines},
};
use std::path::Path;

impl MisyCore {
    /// Discovers providers with explicit provider wait deadlines for integration tests.
    ///
    /// # Errors
    ///
    /// Returns an error when provider discovery, configuration loading, or runtime startup fails.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn discover_with_deadlines(
        paths: MisyPaths,
        bundled_providers: impl AsRef<Path>,
        deadlines: ProviderDeadlines,
    ) -> Result<Self, CoreError> {
        let catalog =
            ProviderCatalog::discover(bundled_providers.as_ref(), &paths.provider_plugins_dir())?;
        Self::build(paths, catalog, deadlines, CoreOptions::default(), None)
    }

    /// Discovers providers with an injected workspace used by integration tests.
    ///
    /// # Errors
    ///
    /// Returns an error when discovery, workspace resolution, configuration, or startup fails.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn discover_in_workspace(
        paths: MisyPaths,
        bundled_providers: impl AsRef<Path>,
        workspace_cwd: impl AsRef<Path>,
    ) -> Result<Self, CoreError> {
        let catalog =
            ProviderCatalog::discover(bundled_providers.as_ref(), &paths.provider_plugins_dir())?;
        Self::build(
            paths,
            catalog,
            ProviderDeadlines::default(),
            CoreOptions::default(),
            Some(workspace_cwd.as_ref().to_path_buf()),
        )
    }

    /// Creates a core from an already-discovered provider catalog.
    ///
    /// # Errors
    ///
    /// Returns an error when persisted configuration cannot be loaded or its runtime cannot start.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn from_catalog(paths: MisyPaths, catalog: ProviderCatalog) -> Result<Self, CoreError> {
        Self::build(
            paths,
            catalog,
            ProviderDeadlines::default(),
            CoreOptions::default(),
            None,
        )
    }

    /// Creates a core from a catalog with explicit provider wait deadlines.
    ///
    /// # Errors
    ///
    /// Returns an error when persisted configuration cannot be loaded or its runtime cannot start.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn from_catalog_with_deadlines(
        paths: MisyPaths,
        catalog: ProviderCatalog,
        deadlines: ProviderDeadlines,
    ) -> Result<Self, CoreError> {
        Self::build(paths, catalog, deadlines, CoreOptions::default(), None)
    }

    /// Discovers providers with immutable client capabilities.
    ///
    /// # Errors
    ///
    /// Returns an error when provider discovery, configuration loading, or runtime startup fails.
    pub fn discover_with_options(
        paths: MisyPaths,
        bundled_providers: impl AsRef<Path>,
        options: CoreOptions,
    ) -> Result<Self, CoreError> {
        let catalog =
            ProviderCatalog::discover(bundled_providers.as_ref(), &paths.provider_plugins_dir())?;
        Self::build(paths, catalog, ProviderDeadlines::default(), options, None)
    }

    /// Creates a test-support core with explicit immutable client capabilities.
    ///
    /// # Errors
    ///
    /// Returns an error when configuration loading or runtime startup fails.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn from_catalog_with_options(
        paths: MisyPaths,
        catalog: ProviderCatalog,
        options: CoreOptions,
    ) -> Result<Self, CoreError> {
        Self::build(paths, catalog, ProviderDeadlines::default(), options, None)
    }
}
