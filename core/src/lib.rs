//! Core logic for the web app manager: reading, writing and deleting
//! `.desktop` launchers that open a URL in a Chromium-based browser's app mode.
//!
//! Only launchers carrying `X-WebApp-Manager=true` are ever listed, edited or
//! deleted, so launchers created by browsers or other tools are never touched.

pub mod browsers;
pub mod desktop;
pub mod icon;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub use browsers::Browser;

/// Marker key written into every launcher this app owns.
pub const MARKER_KEY: &str = "X-WebApp-Manager";
const FILE_PREFIX: &str = "webapp-";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebApp {
    /// Stable identifier, also the launcher file stem without the `webapp-` prefix.
    pub id: String,
    pub name: String,
    pub url: String,
    /// Browser id, see [`browsers::KNOWN`].
    pub browser: String,
    /// Absolute path to the icon file, empty if none.
    pub icon: String,
}

/// What the user fills in on the add/edit form.
#[derive(Debug, Clone, Default)]
pub struct WebAppInput {
    pub name: String,
    pub url: String,
    pub browser: String,
    /// Optional local image chosen by the user. When empty, the site's icon is downloaded.
    pub icon_file: String,
}

/// Why an operation on the store failed. Each variant names one cause, so
/// callers (the GUI) can attach the message to the right form field.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The name is empty.
    EmptyName,
    /// The address isn't a usable http(s) URL; the text says why.
    InvalidUrl(&'static str),
    /// The browser id isn't one of [`browsers::KNOWN`].
    UnknownBrowser(String),
    /// No web app with this id exists (any more).
    NotFound(String),
    /// A user-chosen icon file couldn't be read or decoded.
    Icon { path: PathBuf, reason: String },
    /// Reading or writing `path` failed.
    Io { path: PathBuf, source: io::Error },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::EmptyName => f.write_str("Name is required"),
            Error::InvalidUrl(why) => f.write_str(why),
            Error::UnknownBrowser(id) => write!(f, "Unknown browser '{id}'"),
            Error::NotFound(id) => write!(f, "The web app '{id}' no longer exists"),
            Error::Icon { path, reason } => {
                write!(f, "Could not use the icon {}: {reason}", path.display())
            }
            Error::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Wraps an `io::Error` with the path it concerns, for use in `map_err`.
fn io_at(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Where launchers and icons live.
#[derive(Debug, Clone)]
pub struct Store {
    apps_dir: PathBuf,
    icons_dir: PathBuf,
}

impl Store {
    /// A store using the given folders (tests and tools use scratch folders).
    #[must_use]
    pub fn new(apps_dir: impl Into<PathBuf>, icons_dir: impl Into<PathBuf>) -> Self {
        Store {
            apps_dir: apps_dir.into(),
            icons_dir: icons_dir.into(),
        }
    }

    /// The real locations under `$XDG_DATA_HOME` (default `~/.local/share`).
    #[must_use]
    pub fn user_default() -> Self {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
            });
        Store {
            apps_dir: data.join("applications"),
            icons_dir: data.join("webapp-manager/icons"),
        }
    }

    /// The launcher file for a web app id; the one place that knows the naming scheme.
    #[must_use]
    pub fn launcher_path(&self, id: &str) -> PathBuf {
        self.apps_dir.join(format!("{FILE_PREFIX}{id}.desktop"))
    }

    /// All web apps managed by this tool, sorted by name. Unreadable or
    /// foreign launchers are skipped.
    #[must_use]
    pub fn list(&self) -> Vec<WebApp> {
        let Ok(entries) = fs::read_dir(&self.apps_dir) else {
            return Vec::new();
        };
        let mut apps: Vec<WebApp> = entries
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                let stem = path.file_name()?.to_str()?;
                let id = stem.strip_prefix(FILE_PREFIX)?.strip_suffix(".desktop")?;
                let text = fs::read_to_string(&path).ok()?;
                desktop::parse(id, &text)
            })
            .collect();
        apps.sort_by_key(|a| a.name.to_lowercase());
        apps
    }

    /// Create a new web app, or update the one with `existing_id`.
    ///
    /// May download the site's icon, so call it off the UI thread.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyName`], [`Error::InvalidUrl`] or [`Error::UnknownBrowser`]
    /// for bad input, [`Error::NotFound`] if `existing_id` is gone,
    /// [`Error::Icon`] if the chosen icon file is unusable, and [`Error::Io`]
    /// if the launcher can't be written.
    pub fn save(&self, existing_id: Option<&str>, input: &WebAppInput) -> Result<WebApp> {
        let name = input.name.trim();
        if name.is_empty() {
            return Err(Error::EmptyName);
        }
        let url = normalize_url(&input.url)?;
        let browser = browsers::by_id(&input.browser)
            .ok_or_else(|| Error::UnknownBrowser(input.browser.clone()))?;

        let previous = match existing_id {
            Some(id) => Some(self.find(id)?),
            None => None,
        };
        let id = match &previous {
            Some(p) => p.id.clone(),
            None => self.unique_id(&slugify(name)),
        };

        fs::create_dir_all(&self.apps_dir).map_err(io_at(&self.apps_dir))?;
        fs::create_dir_all(&self.icons_dir).map_err(io_at(&self.icons_dir))?;

        // Icon: a user-chosen file wins; otherwise keep the old icon when the
        // site is unchanged, else download the site's icon (best effort).
        let site_changed = previous.as_ref().is_none_or(|p| p.url != url);
        let chosen = input.icon_file.trim();
        let icon = if !chosen.is_empty() {
            icon::import_file(Path::new(chosen), &self.icons_dir, &id).map_err(|reason| {
                Error::Icon {
                    path: PathBuf::from(chosen),
                    reason,
                }
            })?
        } else if let Some(p) = previous
            .as_ref()
            .filter(|p| !site_changed && !p.icon.is_empty())
        {
            PathBuf::from(&p.icon)
        } else {
            icon::fetch_site_icon(&url, &self.icons_dir, &id).unwrap_or_default()
        };

        let app = WebApp {
            id: id.clone(),
            name: name.to_string(),
            url,
            browser: browser.id.to_string(),
            icon: icon.to_string_lossy().into_owned(),
        };
        let path = self.launcher_path(&id);
        let tmp = path.with_extension("desktop.tmp");
        if let Err(e) =
            fs::write(&tmp, desktop::render(&app, browser)).and_then(|()| fs::rename(&tmp, &path))
        {
            let _ = fs::remove_file(&tmp);
            return Err(io_at(&path)(e));
        }
        // Only now that the launcher points at the new icon is it safe to drop
        // the old ones (or all of them, when there is no icon any more).
        icon::remove_icons(&self.icons_dir, &id, &icon);
        refresh_desktop_database(&self.apps_dir);
        Ok(app)
    }

    /// Delete a web app's launcher and its icon.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] if no such web app exists, [`Error::Io`] if the
    /// launcher can't be deleted.
    pub fn remove(&self, id: &str) -> Result<()> {
        self.find(id)?;
        let path = self.launcher_path(id);
        fs::remove_file(&path).map_err(io_at(&path))?;
        icon::remove_icons(&self.icons_dir, id, Path::new(""));
        refresh_desktop_database(&self.apps_dir);
        Ok(())
    }

    fn find(&self, id: &str) -> Result<WebApp> {
        self.list()
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| Error::NotFound(id.to_string()))
    }

    fn unique_id(&self, base: &str) -> String {
        let base = if base.is_empty() { "app" } else { base };
        let mut id = base.to_string();
        let mut n = 2;
        while self.launcher_path(&id).exists() {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }
}

/// Lowercase ASCII slug: "My Mail!" -> "my-mail".
#[must_use]
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

const ONLY_HTTP: &str = "Only http:// and https:// addresses are supported";

/// Accepts "example.com" or a full URL; only http(s) is allowed.
///
/// # Errors
///
/// [`Error::InvalidUrl`] for an empty address, one with spaces, a non-http(s)
/// scheme, or no host.
pub fn normalize_url(raw: &str) -> Result<String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(Error::InvalidUrl("Address is required"));
    }
    if s.chars().any(char::is_whitespace) {
        return Err(Error::InvalidUrl("An address can't contain spaces"));
    }
    let url = if s.contains("://") {
        s.to_string()
    } else {
        // Bare "host[:port][/path]". Anything else with a colon is another
        // scheme ("javascript:...", "mailto:...") and is rejected.
        let authority = s.split(['/', '?', '#']).next().unwrap_or("");
        if let Some((_, port)) = authority.split_once(':')
            && (port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()))
        {
            return Err(Error::InvalidUrl(ONLY_HTTP));
        }
        format!("https://{s}")
    };
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err(Error::InvalidUrl(ONLY_HTTP));
    }
    if host_of(&url).is_empty() {
        return Err(Error::InvalidUrl("That address has no host"));
    }
    Ok(url)
}

/// The host part of an http(s) URL (no port, no credentials).
#[must_use]
pub fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if let Some(v6) = host_port.strip_prefix('[') {
        return v6.split(']').next().unwrap_or("");
    }
    host_port.split(':').next().unwrap_or("")
}

fn refresh_desktop_database(dir: &Path) {
    let _ = std::process::Command::new("update-desktop-database")
        .arg(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path().join("applications"), dir.path().join("icons"));
        (dir, s)
    }

    fn input(name: &str, url: &str) -> WebAppInput {
        WebAppInput {
            name: name.into(),
            url: url.into(),
            browser: browsers::KNOWN[0].id.into(),
            icon_file: String::new(),
        }
    }

    /// Icon downloads are skipped for .invalid hosts, so tests stay offline.
    #[test]
    fn add_edit_remove_roundtrip() {
        let (_d, s) = store();
        let a = s.save(None, &input("My Mail", "mail.invalid")).unwrap();
        assert_eq!(a.id, "my-mail");
        assert_eq!(a.url, "https://mail.invalid");
        assert_eq!(s.list(), vec![a.clone()]);

        let b = s
            .save(None, &input("My Mail", "https://other.invalid/x"))
            .unwrap();
        assert_eq!(b.id, "my-mail-2");

        let edited = s
            .save(
                Some("my-mail"),
                &input("Mail = Work; #1", "https://mail.invalid/inbox"),
            )
            .unwrap();
        assert_eq!(edited.id, "my-mail");
        let listed = s.list();
        assert_eq!(listed.len(), 2);
        assert!(listed.contains(&edited));

        s.remove("my-mail").unwrap();
        assert_eq!(s.list(), vec![b]);
        assert!(matches!(s.remove("my-mail"), Err(Error::NotFound(id)) if id == "my-mail"));
    }

    #[test]
    fn ignores_foreign_launchers() {
        let (_d, s) = store();
        fs::create_dir_all(&s.apps_dir).unwrap();
        fs::write(
            s.apps_dir.join("webapp-foreign.desktop"),
            "[Desktop Entry]\nType=Application\nName=Foreign\nExec=foo\n",
        )
        .unwrap();
        fs::write(
            s.apps_dir.join("chrome-abc-Default.desktop"),
            "[Desktop Entry]\nName=X\n",
        )
        .unwrap();
        assert_eq!(s.list(), vec![]);
        assert!(matches!(s.remove("foreign"), Err(Error::NotFound(_))));
    }

    #[test]
    fn validation() {
        let (_d, s) = store();
        assert!(matches!(
            s.save(None, &input(" ", "a.invalid")),
            Err(Error::EmptyName)
        ));
        assert!(matches!(
            s.save(None, &input("A", "")),
            Err(Error::InvalidUrl(_))
        ));
        assert!(matches!(
            s.save(None, &input("A", "file:///etc/passwd")),
            Err(Error::InvalidUrl(_))
        ));
        assert!(matches!(
            s.save(None, &input("A", "javascript:alert(1)")),
            Err(Error::InvalidUrl(_))
        ));
        let mut i = input("A", "a.invalid");
        i.browser = "nope".into();
        assert!(matches!(s.save(None, &i), Err(Error::UnknownBrowser(b)) if b == "nope"));
        let mut i = input("A", "a.invalid");
        i.icon_file = "/no/such/icon.png".into();
        assert!(matches!(s.save(None, &i), Err(Error::Icon { .. })));
        assert!(matches!(
            s.save(Some("gone"), &input("A", "a.invalid")),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn new_address_without_icon_drops_old_icon() {
        let (d, s) = store();
        let chosen = d.path().join("chosen.svg");
        fs::write(&chosen, "<svg/>").unwrap();
        let mut i = input("Mail", "mail.invalid");
        i.icon_file = chosen.to_string_lossy().into_owned();
        let a = s.save(None, &i).unwrap();
        assert!(Path::new(&a.icon).exists());

        // Same address, no new image: the icon is kept.
        let kept = s.save(Some(&a.id), &input("Mail", "mail.invalid")).unwrap();
        assert_eq!(kept.icon, a.icon);
        assert!(Path::new(&kept.icon).exists());

        // A newly chosen image replaces the old file.
        std::thread::sleep(std::time::Duration::from_millis(2)); // icon names carry a ms stamp
        let replaced = s.save(Some(&a.id), &i).unwrap();
        assert_ne!(replaced.icon, a.icon);
        assert!(!Path::new(&a.icon).exists() && Path::new(&replaced.icon).exists());

        // Deleting the app deletes its icon.
        let other = s.save(None, &i).unwrap();
        s.remove(&other.id).unwrap();
        assert!(!Path::new(&other.icon).exists());

        // New address with no icon available: the old file is removed, not leaked.
        let moved = s
            .save(Some(&a.id), &input("Mail", "other.invalid"))
            .unwrap();
        assert_eq!(moved.icon, "");
        assert!(!Path::new(&replaced.icon).exists());
    }

    #[test]
    fn failed_launcher_write_keeps_old_icon() {
        let (d, s) = store();
        let png = |name: &str| {
            let p = d.path().join(name);
            fs::write(&p, "<svg/>").unwrap();
            p.to_string_lossy().into_owned()
        };
        let mut i = input("A", "a.invalid");
        i.icon_file = png("one.svg");
        let a = s.save(None, &i).unwrap();

        // A directory where the temp launcher goes makes the write fail.
        let tmp = s.launcher_path(&a.id).with_extension("desktop.tmp");
        fs::create_dir(&tmp).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2)); // icon names carry a ms stamp
        i.icon_file = png("two.svg");
        assert!(matches!(s.save(Some(&a.id), &i), Err(Error::Io { .. })));
        assert!(Path::new(&a.icon).exists());
        assert_eq!(s.list(), vec![a]);
    }

    #[test]
    fn kept_icon_survives_a_differently_spelled_folder() {
        let (d, s) = store();
        let chosen = d.path().join("c.svg");
        fs::write(&chosen, "<svg/>").unwrap();
        let mut i = input("A", "a.invalid");
        i.icon_file = chosen.to_string_lossy().into_owned();
        let a = s.save(None, &i).unwrap();
        // Same folder, reached through a `..` detour, as a symlinked HOME would.
        let odd = Store::new(
            d.path().join("applications"),
            d.path().join("icons/../icons"),
        );
        odd.save(Some(&a.id), &input("B", "a.invalid")).unwrap();
        assert!(Path::new(&a.icon).exists());
    }

    #[test]
    fn io_errors_name_the_path() {
        let (d, _s) = store();
        let blocker = d.path().join("not-a-dir");
        fs::write(&blocker, "").unwrap();
        let s = Store::new(blocker.join("applications"), d.path().join("icons"));
        let err = s.save(None, &input("A", "a.invalid")).unwrap_err();
        assert!(matches!(&err, Error::Io { path, .. } if path.starts_with(&blocker)));
        assert!(err.to_string().contains("not-a-dir"), "{err}");
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn slugs_and_hosts() {
        assert_eq!(slugify("  Hello, World! "), "hello-world");
        assert_eq!(slugify("واتساب"), "");
        assert_eq!(
            host_of("https://user:pw@web.example.com:8443/a?b"),
            "web.example.com"
        );
        assert_eq!(host_of("http://[::1]:3000/x"), "::1");
    }
}

#[cfg(test)]
mod url_tests {
    use super::normalize_url;

    #[test]
    fn urls() {
        assert_eq!(
            normalize_url(" example.com ").unwrap(),
            "https://example.com"
        );
        assert_eq!(
            normalize_url("localhost:8080/x").unwrap(),
            "https://localhost:8080/x"
        );
        assert_eq!(
            normalize_url("http://192.0.2.1:9200").unwrap(),
            "http://192.0.2.1:9200"
        );
        for bad in [
            "javascript:alert(1)",
            "mailto:a@b.c",
            "file:///etc",
            "a b.com",
            "https://",
            "data:text/html,x",
        ] {
            assert!(normalize_url(bad).is_err(), "{bad}");
        }
    }
}
