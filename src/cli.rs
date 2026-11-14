use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use clap_complete::Shell;
use niri_ipc::{Action, OutputAction};

use crate::utils::version;

#[derive(Parser)]
#[command(author, version = version(), about, long_about = None)]
#[command(args_conflicts_with_subcommands = true)]
#[command(subcommand_value_name = "SUBCOMMAND")]
#[command(subcommand_help_heading = "Subcommands")]
pub struct Cli {
    /// Path to config file (default: `$XDG_CONFIG_HOME/niri/config.kdl`).
    ///
    /// This can also be set with the `NIRI_CONFIG` environment variable. If both are set, the
    /// command line argument takes precedence.
    #[arg(short, long)]
    pub config: Option<PathBuf>,
    /// Import environment globally to systemd and D-Bus, run D-Bus services.
    ///
    /// Set this flag in a systemd service started by your display manager, or when running
    /// manually as your main compositor instance. Do not set when running as a nested window, or
    /// on a TTY as your non-main compositor instance, to avoid messing up the global environment.
    #[arg(long)]
    pub session: bool,
    /// Command to run upon compositor startup.
    #[arg(last = true)]
    pub command: Vec<OsString>,

    #[command(subcommand)]
    pub subcommand: Option<Sub>,
}

#[derive(Subcommand)]
pub enum Sub {
    /// Communicate with the running niri instance.
    Msg {
        #[command(subcommand)]
        msg: Msg,
        /// Format output as JSON.
        #[arg(short, long)]
        json: bool,
        /// Print the IPC request as JSON instead of sending it.
        #[arg(long)]
        print_request: bool,
    },
    /// Validate the config file.
    Validate {
        /// Path to config file (default: `$XDG_CONFIG_HOME/niri/config.kdl`).
        ///
        /// This can also be set with the `NIRI_CONFIG` environment variable. If both are set, the
        /// command line argument takes precedence.
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
    /// Cause a panic to check if the backtraces are good.
    Panic,
    /// Generate shell completions.
    Completions { shell: CompletionShell },
}

#[derive(Subcommand)]
pub enum Msg {
    /// List connected outputs.
    Outputs,
    /// List workspaces.
    Workspaces,
    /// List open windows.
    Windows,
    /// List open layer-shell surfaces.
    Layers,
    /// Get the configured keyboard layouts.
    KeyboardLayouts,
    /// Print information about the focused output.
    FocusedOutput,
    /// Print information about the focused window.
    FocusedWindow,
    /// Pick a window with the mouse and print information about it.
    PickWindow,
    /// Pick a color from the screen with the mouse.
    PickColor,
    /// Perform an action.
    Action {
        #[command(subcommand)]
        action: Action,
    },
    /// Change output configuration temporarily.
    ///
    /// The configuration is changed temporarily and not saved into the config file. If the output
    /// configuration subsequently changes in the config file, these temporary changes will be
    /// forgotten.
    Output {
        /// Output name.
        ///
        /// Run `niri msg outputs` to see the output names.
        #[arg()]
        output: String,
        /// Configuration to apply.
        #[command(subcommand)]
        action: OutputAction,
    },
    /// Start continuously receiving events from the compositor.
    EventStream,
    /// Print the version of the running niri instance.
    Version,
    /// Request an error from the running niri instance.
    RequestError,
    /// Print the overview state.
    OverviewState,
    /// List screencasts.
    Casts,
    /// List the views (pan/zoom) of canvas-mode workspaces.
    CanvasViews,
    /// Send a raw JSON request to the compositor, reading from stdin.
    RawRequest,
}

#[derive(Clone, Debug, clap::ValueEnum)]
pub enum CompletionShell {
    Bash,
    Elvish,
    Fish,
    PowerShell,
    Zsh,
    Nushell,
}

impl TryFrom<CompletionShell> for Shell {
    type Error = &'static str;

    fn try_from(shell: CompletionShell) -> Result<Self, Self::Error> {
        match shell {
            CompletionShell::Bash => Ok(Shell::Bash),
            CompletionShell::Elvish => Ok(Shell::Elvish),
            CompletionShell::Fish => Ok(Shell::Fish),
            CompletionShell::PowerShell => Ok(Shell::PowerShell),
            CompletionShell::Zsh => Ok(Shell::Zsh),
            CompletionShell::Nushell => Err("Nushell should be handled separately"),
        }
    }
}

// rice: CLI flags of the fork's IPC additions.
#[cfg(test)]
mod tests {
    use super::*;

    fn action(args: &[&str]) -> Action {
        let args: Vec<String> = ["niri", "msg", "action"]
            .iter()
            .chain(args)
            .map(|s| s.to_string())
            .collect();
        // The derived parser of the big `Action` enum needs more stack than a test thread has in
        // debug builds.
        let cli = std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(move || Cli::try_parse_from(args).unwrap())
            .unwrap()
            .join()
            .unwrap();
        let Some(Sub::Msg {
            msg: Msg::Action { action },
            ..
        }) = cli.subcommand
        else {
            panic!("not an action");
        };
        action
    }

    #[test]
    fn screenshot_delivery_flags() {
        let Action::ScreenshotScreen {
            copy_to_clipboard,
            notify,
            ..
        } = action(&["screenshot-screen"])
        else {
            panic!()
        };
        assert!(copy_to_clipboard && notify);

        let Action::ScreenshotScreen {
            write_to_disk,
            copy_to_clipboard,
            notify,
            ..
        } = action(&[
            "screenshot-screen",
            "--write-to-disk",
            "false",
            "--copy-to-clipboard",
            "false",
            "--notify",
            "false",
        ])
        else {
            panic!()
        };
        assert!(!write_to_disk && !copy_to_clipboard && !notify);

        let Action::ScreenshotWindow { notify, .. } =
            action(&["screenshot-window", "--notify", "false"])
        else {
            panic!()
        };
        assert!(!notify);
    }
}
