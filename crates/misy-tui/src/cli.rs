//! Command-line argument parsing for the Misy executable.

use misy_core::{ConfigError, MisyPaths};
use std::{env, error::Error, ffi::OsString, fmt, path::PathBuf};

#[derive(Debug)]
pub(crate) struct Arguments {
    config_root: Option<PathBuf>,
}

impl Arguments {
    pub(crate) fn from_env() -> Result<Self, ArgumentsError> {
        Self::parse(env::args_os().skip(1))
    }

    pub(crate) fn paths(&self) -> Result<MisyPaths, ConfigError> {
        match &self.config_root {
            Some(root) => Ok(MisyPaths::from_root(root.clone())),
            None => MisyPaths::from_home(),
        }
    }

    fn parse<I, T>(values: I) -> Result<Self, ArgumentsError>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString>,
    {
        let mut values = values.into_iter().map(Into::into);
        let mut arguments = Self { config_root: None };

        while let Some(argument) = values.next() {
            if argument == "-c" {
                let root = values.next().ok_or(ArgumentsError::MissingConfigPath)?;
                arguments.set_config_root(root)?;
                continue;
            }
            if let Some(root) = argument
                .to_str()
                .and_then(|value| value.strip_prefix("--config="))
            {
                arguments.set_config_root(OsString::from(root))?;
                continue;
            }
            return Err(ArgumentsError::UnexpectedArgument(argument));
        }

        Ok(arguments)
    }

    fn set_config_root(&mut self, root: OsString) -> Result<(), ArgumentsError> {
        if root.is_empty() {
            return Err(ArgumentsError::EmptyConfigPath);
        }
        if self.config_root.is_some() {
            return Err(ArgumentsError::DuplicateConfigPath);
        }
        self.config_root = Some(PathBuf::from(root));
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ArgumentsError {
    MissingConfigPath,
    EmptyConfigPath,
    DuplicateConfigPath,
    UnexpectedArgument(OsString),
}

impl fmt::Display for ArgumentsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingConfigPath => formatter.write_str("-c requires a configuration directory"),
            Self::EmptyConfigPath => formatter.write_str("configuration directory cannot be empty"),
            Self::DuplicateConfigPath => {
                formatter.write_str("configuration directory was specified more than once")
            }
            Self::UnexpectedArgument(argument) => {
                write!(
                    formatter,
                    "unexpected argument: {}",
                    argument.to_string_lossy()
                )
            }
        }
    }
}

impl Error for ArgumentsError {}

#[cfg(test)]
mod tests {
    use super::{Arguments, ArgumentsError};
    use misy_core::MisyPaths;

    #[test]
    fn no_option_uses_the_home_directory_default() {
        let arguments = Arguments::parse(Vec::<String>::new()).expect("parse empty arguments");

        assert_eq!(
            arguments.paths().expect("resolve home directory"),
            MisyPaths::from_home().expect("resolve expected home directory")
        );
    }

    #[test]
    fn short_config_option_selects_an_explicit_misy_directory() {
        let arguments =
            Arguments::parse(["-c", "/tmp/alternate-misy"]).expect("parse short option");

        assert_eq!(
            arguments.paths().expect("resolve explicit directory"),
            MisyPaths::from_root("/tmp/alternate-misy")
        );
    }

    #[test]
    fn long_config_option_accepts_an_equals_value_with_spaces() {
        let arguments = Arguments::parse(["--config=/tmp/misy profile"])
            .expect("parse long option with spaces");

        assert_eq!(
            arguments.paths().expect("resolve explicit directory"),
            MisyPaths::from_root("/tmp/misy profile")
        );
    }

    #[test]
    fn invalid_config_options_are_rejected() {
        let cases = [
            (vec!["-c"], ArgumentsError::MissingConfigPath),
            (vec!["-c", ""], ArgumentsError::EmptyConfigPath),
            (vec!["--config="], ArgumentsError::EmptyConfigPath),
            (
                vec!["-c", "one", "--config=two"],
                ArgumentsError::DuplicateConfigPath,
            ),
            (
                vec!["--config", "path"],
                ArgumentsError::UnexpectedArgument("--config".into()),
            ),
        ];

        for (values, expected) in cases {
            assert_eq!(
                Arguments::parse(values).expect_err("must reject arguments"),
                expected
            );
        }
    }
}
