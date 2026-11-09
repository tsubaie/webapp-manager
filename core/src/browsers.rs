//! Chromium-based browsers that support `--app=<url>` windows.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Browser {
    /// Stored in the launcher as `X-WebApp-Browser`.
    pub id: &'static str,
    pub name: &'static str,
    /// Executables to try, first found wins.
    pub commands: &'static [&'static str],
}

pub const KNOWN: &[Browser] = &[
    Browser {
        id: "chrome",
        name: "Google Chrome",
        commands: &["google-chrome", "google-chrome-stable"],
    },
    Browser {
        id: "chromium",
        name: "Chromium",
        commands: &["chromium", "chromium-browser"],
    },
    Browser {
        id: "brave",
        name: "Brave",
        commands: &["brave-browser", "brave"],
    },
    Browser {
        id: "vivaldi",
        name: "Vivaldi",
        commands: &["vivaldi", "vivaldi-stable"],
    },
    Browser {
        id: "edge",
        name: "Microsoft Edge",
        commands: &[
            "microsoft-edge",
            "microsoft-edge-stable",
            "microsoft-edge-dev",
        ],
    },
];

#[must_use]
pub fn by_id(id: &str) -> Option<&'static Browser> {
    KNOWN.iter().find(|b| b.id == id)
}

impl Browser {
    /// The first of this browser's executables found on `$PATH`, if any.
    #[must_use]
    pub fn installed_command(&self) -> Option<&'static str> {
        self.commands.iter().copied().find(|c| on_path(c))
    }

    /// The command to put in a launcher; falls back to the primary name so a
    /// launcher still gets written if the browser is installed later.
    #[must_use]
    pub fn command(&self) -> &'static str {
        self.installed_command().unwrap_or(self.commands[0])
    }
}

/// Browsers installed on this machine.
#[must_use]
pub fn available() -> Vec<&'static Browser> {
    KNOWN
        .iter()
        .filter(|b| b.installed_command().is_some())
        .collect()
}

fn on_path(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| is_executable(&dir.join(cmd))))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
