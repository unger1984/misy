//! Construction paths that bind immutable frontend capabilities to the core.

use super::{CoreError, CoreOptions, MisyCore};
use crate::{
    MisyPaths,
    providers::{ProviderCatalog, ProviderDeadlines},
};
use std::path::Path;

impl MisyCore {
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
        Self::build(paths, catalog, ProviderDeadlines::default(), options)
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
        Self::build(paths, catalog, ProviderDeadlines::default(), options)
    }
}
