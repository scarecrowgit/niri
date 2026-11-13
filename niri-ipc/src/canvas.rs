//! Types for the rice fork's per-workspace infinite canvas mode.
//!
//! A workspace is either in [`WorkspaceMode::Strip`] (stock niri scrollable tiling) or in
//! [`WorkspaceMode::Canvas`], where all normal windows float on an unbounded 2D plane and the
//! output shows a pannable, zoomable viewport onto it.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Window arrangement mode of a workspace.
#[derive(Serialize, Deserialize, Debug, Default, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub enum WorkspaceMode {
    /// Stock niri scrollable tiling (columns on an infinite horizontal strip).
    #[default]
    Strip,
    /// Infinite canvas: windows float on an unbounded plane, viewed through a pannable and
    /// zoomable viewport.
    Canvas,
}

impl FromStr for WorkspaceMode {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "strip" => Ok(Self::Strip),
            "canvas" => Ok(Self::Canvas),
            _ => Err(r#"invalid workspace mode, can be "strip" or "canvas""#),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_workspace_mode() {
        assert_eq!("strip".parse(), Ok(WorkspaceMode::Strip));
        assert_eq!("canvas".parse(), Ok(WorkspaceMode::Canvas));
        assert!("Canvas".parse::<WorkspaceMode>().is_err());
    }

    #[test]
    fn workspace_mode_defaults_for_old_clients() {
        // Old servers do not send `mode`; new clients must still parse their replies.
        #[derive(Deserialize)]
        struct Probe {
            #[serde(default)]
            mode: WorkspaceMode,
        }
        let probe: Probe = serde_json::from_str("{}").unwrap();
        assert_eq!(probe.mode, WorkspaceMode::Strip);
    }
}
