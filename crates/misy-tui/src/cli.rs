//! Command-line argument parsing for the Misy executable.

use misy_core::{ConfigError, MisyPaths};
use misy_tui::SessionStart;
use std::{env, error::Error, ffi::OsString, fmt, path::PathBuf};

#[derive(Debug)]
pub(crate) struct Arguments {
    config_root: Option<PathBuf>,
    session_start: SessionStart,
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

    pub(crate) fn session_start(&self) -> SessionStart {
        self.session_start.clone()
    }

    fn parse<I, T>(values: I) -> Result<Self, ArgumentsError>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString>,
    {
        let mut values = values.into_iter().map(Into::into).peekable();
        let mut arguments = Self {
            config_root: None,
            session_start: SessionStart::Fresh,
        };

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
            if argument == "--continue" {
                arguments.set_session_start(SessionStart::ResumeLatest)?;
                continue;
            }
            if argument == "--resume" {
                let id = values
                    .next_if(|value| !value.to_string_lossy().starts_with('-'))
                    .map(|value| value.to_string_lossy().into_owned());
                let start = id.map_or(SessionStart::ResumePicker, SessionStart::ResumeId);
                arguments.set_session_start(start)?;
                continue;
            }
            if let Some(id) = argument
                .to_str()
                .and_then(|value| value.strip_prefix("--resume="))
            {
                let start = if id.is_empty() {
                    SessionStart::ResumePicker
                } else {
                    SessionStart::ResumeId(id.to_owned())
                };
                arguments.set_session_start(start)?;
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

    fn set_session_start(&mut self, start: SessionStart) -> Result<(), ArgumentsError> {
        if self.session_start != SessionStart::Fresh {
            return Err(ArgumentsError::ConflictingSessionOptions);
        }
        self.session_start = start;
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ArgumentsError {
    MissingConfigPath,
    EmptyConfigPath,
    DuplicateConfigPath,
    ConflictingSessionOptions,
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
            Self::ConflictingSessionOptions => {
                formatter.write_str("session resume options are mutually exclusive")
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
    use misy_tui::SessionStart;

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

    #[test]
    fn parses_session_start_options() {
        let cases = [
            (vec!["--continue"], SessionStart::ResumeLatest),
            (vec!["--resume"], SessionStart::ResumePicker),
            (
                vec!["--resume", "abc123"],
                SessionStart::ResumeId("abc123".to_owned()),
            ),
            (
                vec!["--resume=def456"],
                SessionStart::ResumeId("def456".to_owned()),
            ),
        ];
        for (values, expected) in cases {
            assert_eq!(
                Arguments::parse(values)
                    .expect("parse session option")
                    .session_start(),
                expected
            );
        }
    }

    #[test]
    fn rejects_conflicting_session_start_options() {
        assert_eq!(
            Arguments::parse(["--continue", "--resume"])
                .expect_err("session options must conflict"),
            ArgumentsError::ConflictingSessionOptions
        );
    }
}
