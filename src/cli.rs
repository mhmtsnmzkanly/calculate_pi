//! Command-line argument parser and validation rules.
use crate::math::MAX_BATCH_DIGITS;
use crate::verify::{Coverage, DEFAULT_SAMPLES};
use thiserror::Error;

pub const DEFAULT_OUTPUT: &str = "pi_record.json";

pub const HELP: &str = "\
Pi Calculator

Usage:
  ./pi_hesaplayici [Options]

Options:
  --gui              Launch the desktop GUI (default when built with `gui`)
  --tui              Launch the terminal TUI
  -o, --output <Path> Path to the computation record file
  --headless         Run without an interface, printing progress every 10 seconds
  --until <N>        Stop and save once N hex digits are certified
  --at <Index>       Compute and cross-verify hex digits starting at 0-based index
  --count <N>        Number of hex digits for --at (1..=20, default 1)
  --reverify <Path>  Re-verify a legacy record and write it to --output
  --verify <Path>    Verify a record against the independent Bellard formula
  --samples <N|all>  --verify scope: N random windows (default 32) or all digits
  -h, --help         Show this help text";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    #[cfg(feature = "gui")]
    Gui,
    Tui,
    Headless,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliAction {
    Help,
    Run {
        mode: Mode,
        output: String,
        until: Option<u64>,
    },
    Query {
        index: u64,
        count: usize,
    },
    Reverify {
        input: String,
        output: String,
    },
    Verify {
        input: String,
        coverage: Coverage,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CliError {
    #[error("unknown option: {0}")]
    Unknown(String),
    #[error("option {0} requires a value")]
    MissingValue(String),
    #[error("options --gui, --tui and --headless are mutually exclusive")]
    ConflictingModes,
    #[error("--reverify requires --output")]
    ReverifyNeedsOutput,
    #[error("--reverify cannot be combined with an interface option or --until")]
    ReverifyWithMode,
    #[error("--verify cannot be combined with other commands or options")]
    VerifyConflict,
    #[error("--samples requires --verify")]
    SamplesNeedsVerify,
    #[error("--at cannot be combined with other commands or mode options")]
    QueryConflict,
    #[error("--count requires --at")]
    CountNeedsAt,
    #[error("invalid value for {flag}: {value}")]
    InvalidNumber { flag: String, value: String },
    #[error("this build has no GUI; rebuild with the `gui` feature or use --tui")]
    GuiUnavailable,
}

#[cfg(feature = "gui")]
const DEFAULT_MODE: Mode = Mode::Gui;
#[cfg(not(feature = "gui"))]
const DEFAULT_MODE: Mode = Mode::Tui;

#[cfg(feature = "gui")]
fn gui_mode() -> Result<Mode, CliError> {
    Ok(Mode::Gui)
}
#[cfg(not(feature = "gui"))]
fn gui_mode() -> Result<Mode, CliError> {
    Err(CliError::GuiUnavailable)
}

fn set_mode(slot: &mut Option<Mode>, mode: Mode) -> Result<(), CliError> {
    match slot.replace(mode) {
        Some(_) => Err(CliError::ConflictingModes),
        None => Ok(()),
    }
}

fn option_value(flag: &str, args: &mut impl Iterator<Item = String>) -> Result<String, CliError> {
    args.next()
        .filter(|value| !value.starts_with('-'))
        .ok_or_else(|| CliError::MissingValue(flag.to_string()))
}

fn parse_until(flag: &str, value: String) -> Result<u64, CliError> {
    match value.parse() {
        Ok(target) if target > 0 => Ok(target),
        _ => Err(CliError::InvalidNumber {
            flag: flag.to_string(),
            value,
        }),
    }
}

fn parse_index(flag: &str, value: String) -> Result<u64, CliError> {
    value.parse().map_err(|_| CliError::InvalidNumber {
        flag: flag.to_string(),
        value,
    })
}

fn parse_count(flag: &str, value: String) -> Result<usize, CliError> {
    match value.parse::<usize>() {
        Ok(count) if (1..=MAX_BATCH_DIGITS).contains(&count) => Ok(count),
        _ => Err(CliError::InvalidNumber {
            flag: flag.to_string(),
            value,
        }),
    }
}

fn parse_coverage(flag: &str, value: String) -> Result<Coverage, CliError> {
    if value == "all" {
        return Ok(Coverage::Full);
    }
    value
        .parse()
        .map(Coverage::Samples)
        .map_err(|_| CliError::InvalidNumber {
            flag: flag.to_string(),
            value,
        })
}

pub fn parse<I, S>(args: I) -> Result<CliAction, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut args = args.into_iter().map(Into::into);
    let mut mode = None;
    let mut output = None;
    let mut reverify = None;
    let mut verify = None;
    let mut coverage = None;
    let mut until = None;
    let mut at = None;
    let mut count = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--gui" => set_mode(&mut mode, gui_mode()?)?,
            "--tui" => set_mode(&mut mode, Mode::Tui)?,
            "--headless" => set_mode(&mut mode, Mode::Headless)?,
            "-o" | "--output" => output = Some(option_value(&arg, &mut args)?),
            "--reverify" => reverify = Some(option_value(&arg, &mut args)?),
            "--verify" => verify = Some(option_value(&arg, &mut args)?),
            "--samples" => coverage = Some(parse_coverage(&arg, option_value(&arg, &mut args)?)?),
            "--until" => until = Some(parse_until(&arg, option_value(&arg, &mut args)?)?),
            "--at" => at = Some(parse_index(&arg, option_value(&arg, &mut args)?)?),
            "--count" => count = Some(parse_count(&arg, option_value(&arg, &mut args)?)?),
            "-h" | "--help" => return Ok(CliAction::Help),
            _ => return Err(CliError::Unknown(arg)),
        }
    }
    if let Some(index) = at {
        if mode.is_some()
            || output.is_some()
            || reverify.is_some()
            || verify.is_some()
            || coverage.is_some()
            || until.is_some()
        {
            return Err(CliError::QueryConflict);
        }
        return Ok(CliAction::Query {
            index,
            count: count.unwrap_or(1),
        });
    }
    if count.is_some() {
        return Err(CliError::CountNeedsAt);
    }
    if let Some(input) = verify {
        if mode.is_some() || output.is_some() || reverify.is_some() || until.is_some() {
            return Err(CliError::VerifyConflict);
        }
        return Ok(CliAction::Verify {
            input,
            coverage: coverage.unwrap_or(Coverage::Samples(DEFAULT_SAMPLES)),
        });
    }
    if coverage.is_some() {
        return Err(CliError::SamplesNeedsVerify);
    }
    let Some(input) = reverify else {
        return Ok(CliAction::Run {
            mode: mode.unwrap_or(DEFAULT_MODE),
            output: output.unwrap_or_else(|| DEFAULT_OUTPUT.to_string()),
            until,
        });
    };
    if mode.is_some() || until.is_some() {
        return Err(CliError::ReverifyWithMode);
    }
    let output = output.ok_or(CliError::ReverifyNeedsOutput)?;
    Ok(CliAction::Reverify { input, output })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "gui")]
    #[test]
    fn rejects_conflicting_modes() {
        assert_eq!(parse(["--gui", "--tui"]), Err(CliError::ConflictingModes));
    }

    #[cfg(not(feature = "gui"))]
    #[test]
    fn gui_flag_is_rejected_without_the_feature() {
        assert_eq!(parse(["--gui"]), Err(CliError::GuiUnavailable));
    }

    #[test]
    fn rejects_option_value() {
        assert!(matches!(
            parse(["--output", "--tui"]),
            Err(CliError::MissingValue(_))
        ));
        assert_eq!(parse(["-o"]), Err(CliError::MissingValue("-o".to_string())));
    }

    #[test]
    fn parses_run_with_defaults() {
        assert_eq!(
            parse(Vec::<String>::new()),
            Ok(CliAction::Run {
                mode: DEFAULT_MODE,
                output: DEFAULT_OUTPUT.to_string(),
                until: None
            })
        );
    }

    #[test]
    fn parses_headless_with_target() {
        assert_eq!(
            parse(["--headless", "--until", "500"]),
            Ok(CliAction::Run {
                mode: Mode::Headless,
                output: DEFAULT_OUTPUT.to_string(),
                until: Some(500)
            })
        );
    }

    #[test]
    fn rejects_bad_targets() {
        for value in ["0", "x", "1.5"] {
            assert!(matches!(
                parse(["--until", value]),
                Err(CliError::InvalidNumber { .. })
            ));
        }
    }

    #[test]
    fn verify_defaults_to_sampling() {
        assert_eq!(
            parse(["--verify", "a.json"]),
            Ok(CliAction::Verify {
                input: "a.json".to_string(),
                coverage: Coverage::Samples(DEFAULT_SAMPLES)
            })
        );
    }

    #[test]
    fn verify_accepts_full_coverage() {
        assert_eq!(
            parse(["--verify", "a.json", "--samples", "all"]),
            Ok(CliAction::Verify {
                input: "a.json".to_string(),
                coverage: Coverage::Full
            })
        );
    }

    #[test]
    fn verify_rejects_other_options() {
        for extra in ["--tui", "--headless"] {
            assert_eq!(
                parse(["--verify", "a.json", extra]),
                Err(CliError::VerifyConflict)
            );
        }
        assert_eq!(parse(["--samples", "3"]), Err(CliError::SamplesNeedsVerify));
    }

    #[test]
    fn reverify_with_target_is_rejected() {
        assert_eq!(
            parse(["--reverify", "a", "-o", "b", "--until", "3"]),
            Err(CliError::ReverifyWithMode)
        );
    }

    #[test]
    fn parses_reverify() {
        assert_eq!(
            parse(["--reverify", "old.json", "--output", "new.json"]),
            Ok(CliAction::Reverify {
                input: "old.json".to_string(),
                output: "new.json".to_string()
            })
        );
    }

    #[test]
    fn reverify_accepts_explicit_default_output_name() {
        assert_eq!(
            parse(["--reverify", "old.json", "--output", DEFAULT_OUTPUT]),
            Ok(CliAction::Reverify {
                input: "old.json".to_string(),
                output: DEFAULT_OUTPUT.to_string()
            })
        );
    }

    #[test]
    fn reverify_without_output_is_rejected() {
        assert_eq!(
            parse(["--reverify", "old.json"]),
            Err(CliError::ReverifyNeedsOutput)
        );
    }

    #[test]
    fn reverify_with_mode_is_rejected() {
        assert_eq!(
            parse(["--reverify", "old.json", "-o", "new.json", "--tui"]),
            Err(CliError::ReverifyWithMode)
        );
    }

    #[test]
    fn help_is_an_action_not_default_gui() {
        assert_eq!(parse(["--help"]), Ok(CliAction::Help));
    }

    #[test]
    fn parses_query_at_and_count() {
        assert_eq!(
            parse(["--at", "0"]),
            Ok(CliAction::Query { index: 0, count: 1 })
        );
        assert_eq!(
            parse(["--at", "100000", "--count", "8"]),
            Ok(CliAction::Query {
                index: 100_000,
                count: 8
            })
        );
        assert_eq!(parse(["--count", "4"]), Err(CliError::CountNeedsAt));
        assert_eq!(parse(["--at", "10", "--tui"]), Err(CliError::QueryConflict));
        for bad_count in ["0", "21", "abc"] {
            assert!(matches!(
                parse(["--at", "0", "--count", bad_count]),
                Err(CliError::InvalidNumber { .. })
            ));
        }
    }
}
