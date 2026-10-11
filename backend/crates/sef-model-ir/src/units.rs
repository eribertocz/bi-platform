use crate::diagnostic::{Diagnostic, ErrorCode};
use crate::identity::valid_segment;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Unit {
    Currency { iso4217: String },
    Physical { ucum: String },
    Ratio,
    Percent,
    Count,
    Custom { namespace: String, code: String },
}

impl Unit {
    pub fn validate(&self) -> Result<(), Diagnostic> {
        let valid = match self {
            Self::Currency { iso4217 } => {
                iso4217.len() == 3 && iso4217.bytes().all(|b| b.is_ascii_uppercase())
            }
            Self::Physical { ucum } => {
                !ucum.is_empty() && ucum.bytes().all(|b| b.is_ascii_graphic())
            }
            Self::Custom { namespace, code } => {
                valid_segment(namespace) && !code.is_empty() && !code.chars().any(char::is_control)
            }
            Self::Ratio | Self::Percent | Self::Count => true,
        };
        if valid {
            Ok(())
        } else {
            Err(Diagnostic::error(
                ErrorCode::IncompatibleUnits,
                None,
                "Malformed declared unit identifier",
            ))
        }
    }
}

pub fn require_same_units(left: &Option<Unit>, right: &Option<Unit>) -> Result<(), Diagnostic> {
    if left == right {
        Ok(())
    } else {
        Err(Diagnostic::error(
            ErrorCode::IncompatibleUnits,
            None,
            "Definition requires an explicit unit conversion",
        ))
    }
}
