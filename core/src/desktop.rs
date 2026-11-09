//! Rendering and parsing of the `.desktop` launchers this app owns.

use crate::{Browser, MARKER_KEY, WebApp};

const URL_KEY: &str = "X-WebApp-URL";
const BROWSER_KEY: &str = "X-WebApp-Browser";

/// The full launcher file for `app`.
#[must_use]
pub fn render(app: &WebApp, browser: &Browser) -> String {
    let class = wm_class(&app.id);
    let exec = [
        browser.command().to_string(),
        format!("--app={}", app.url),
        format!("--class={class}"),
    ]
    .iter()
    .map(|a| exec_quote(a))
    .collect::<Vec<_>>()
    .join(" ");

    let mut s = String::from("[Desktop Entry]\nType=Application\nVersion=1.0\n");
    push(&mut s, "Name", &app.name);
    push(
        &mut s,
        "Comment",
        &format!("Web app for {}", crate::host_of(&app.url)),
    );
    push(&mut s, "Exec", &exec);
    if !app.icon.is_empty() {
        push(&mut s, "Icon", &app.icon);
    }
    push(&mut s, "StartupWMClass", &class);
    s.push_str("StartupNotify=true\nTerminal=false\nCategories=Network;WebBrowser;\n");
    push(&mut s, MARKER_KEY, "true");
    push(&mut s, URL_KEY, &app.url);
    push(&mut s, BROWSER_KEY, browser.id);
    s
}

/// Reads a launcher back. Returns `None` unless it carries our marker.
#[must_use]
pub fn parse(id: &str, text: &str) -> Option<WebApp> {
    let mut in_main = false;
    let (mut name, mut url, mut browser, mut icon, mut ours) = (None, None, None, None, false);
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = unescape(v.trim());
        match k.trim() {
            "Name" => name = Some(v),
            "Icon" => icon = Some(v),
            URL_KEY => url = Some(v),
            BROWSER_KEY => browser = Some(v),
            k if k == MARKER_KEY => ours = v == "true",
            _ => {}
        }
    }
    if !ours {
        return None;
    }
    Some(WebApp {
        id: id.to_string(),
        name: name?,
        url: url?,
        browser: browser.unwrap_or_else(|| "chrome".into()),
        icon: icon.unwrap_or_default(),
    })
}

/// Window class used to group the web app's windows under its own icon.
#[must_use]
pub fn wm_class(id: &str) -> String {
    format!("webapp-{id}")
}

fn push(s: &mut String, key: &str, value: &str) {
    s.push_str(key);
    s.push('=');
    s.push_str(&escape(value));
    s.push('\n');
}

/// Escapes a string value per the Desktop Entry spec.
fn escape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    if out.starts_with(' ') {
        out.replace_range(0..1, "\\s");
    }
    out
}

fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            // `\\` is a backslash; an unknown escape or a trailing `\` is kept as-is.
            Some(o) => {
                out.push('\\');
                if o != '\\' {
                    out.push(o);
                }
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Quotes one `Exec` argument: wraps it in double quotes when needed, escapes
/// the characters the spec reserves inside quotes, and doubles `%` so field
/// codes are never triggered by percent-encoded URLs.
fn exec_quote(arg: &str) -> String {
    let arg = arg.replace('%', "%%");
    let needs_quotes = arg.is_empty()
        || arg.chars().any(|c| {
            c.is_whitespace()
                || matches!(
                    c,
                    '"' | '\''
                        | '\\'
                        | '>'
                        | '<'
                        | '~'
                        | '|'
                        | '&'
                        | ';'
                        | '$'
                        | '*'
                        | '?'
                        | '#'
                        | '('
                        | ')'
                        | '`'
                )
        });
    if !needs_quotes {
        return arg;
    }
    let mut out = String::from("\"");
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browsers::KNOWN;

    fn app(name: &str, url: &str) -> WebApp {
        WebApp {
            id: "x".into(),
            name: name.into(),
            url: url.into(),
            browser: "chrome".into(),
            icon: "/i/x.png".into(),
        }
    }

    #[test]
    fn roundtrip_with_special_characters() {
        let a = app(
            " Tricky\\Name\twith\nlines",
            "https://e.com/a?q=1&b=%20$x#frag",
        );
        let text = render(&a, &KNOWN[0]);
        assert_eq!(parse("x", &text), Some(a));
    }

    #[test]
    fn exec_line_is_safe() {
        let text = render(&app("A", "https://e.com/a?q=%20&x=$HOME;rm"), &KNOWN[0]);
        let exec = text.lines().find(|l| l.starts_with("Exec=")).unwrap();
        // Quoted, % doubled, $ escaped; in the file the backslash itself is escaped again.
        assert!(
            exec.contains(r#""--app=https://e.com/a?q=%%20&x=\\$HOME;rm""#),
            "{exec}"
        );
        assert!(exec.ends_with("--class=webapp-x"), "{exec}");
    }

    #[test]
    fn requires_marker() {
        assert_eq!(
            parse("x", "[Desktop Entry]\nName=A\nX-WebApp-URL=https://a\n"),
            None
        );
        assert_eq!(
            parse(
                "x",
                "[Desktop Entry]\nName=A\n[Other]\nX-WebApp-Manager=true\nX-WebApp-URL=https://a\n"
            ),
            None
        );
    }
}
