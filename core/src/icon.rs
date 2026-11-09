//! Getting an icon for a web app: download the site's own icon, or import a
//! file the user picked. Raster images are normalised to PNG (max 256 px);
//! SVGs are stored as-is.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_PAGE: u64 = 2 * 1024 * 1024;
const MAX_IMAGE: u64 = 5 * 1024 * 1024;
const MAX_SIDE: u32 = 256;
/// Stop trying further icon candidates after this; each request has its own 8 s timeout.
const FETCH_BUDGET: Duration = Duration::from_secs(20);
/// Decode limits for untrusted images: a small file can claim huge dimensions.
const MAX_DECODE_SIDE: u32 = 4096;
const MAX_DECODE_ALLOC: u64 = 64 * 1024 * 1024;

/// Downloads the best icon the site advertises (falling back to
/// `/favicon.ico`) into `dir`. Returns `None` if nothing usable was found.
#[must_use]
pub fn fetch_site_icon(url: &str, dir: &Path, id: &str) -> Option<PathBuf> {
    let host = crate::host_of(url).to_ascii_lowercase();
    // Reserved TLDs never resolve; skipping them keeps tests offline.
    if matches!(host.rsplit('.').next(), Some("invalid" | "test")) {
        return None;
    }
    let deadline = Instant::now() + FETCH_BUDGET;
    let agent = agent();
    let origin = origin_of(url);

    let mut candidates = Vec::new();
    if let Some(html) = get(&agent, url, MAX_PAGE) {
        let html = String::from_utf8_lossy(&html);
        candidates.extend(icon_links(&html).into_iter().map(|h| resolve(url, &h)));
    }
    candidates.push(format!("{origin}/apple-touch-icon.png"));
    candidates.push(format!("{origin}/favicon.ico"));

    candidates
        .iter()
        .take_while(|_| Instant::now() < deadline)
        .filter_map(|c| get(&agent, c, MAX_IMAGE))
        .find_map(|bytes| store(&bytes, dir, id).ok())
}

/// Copies a user-chosen image into `dir`. Accepts a path or a `file://` URL.
///
/// # Errors
///
/// A readable reason if the file is missing, larger than 5 MB, or not an image.
pub fn import_file(path: &Path, dir: &Path, id: &str) -> Result<PathBuf, String> {
    let s = path.to_string_lossy();
    let path = match s.strip_prefix("file://") {
        Some(p) => PathBuf::from(percent_decode(p)),
        None => path.to_path_buf(),
    };
    let meta = fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_IMAGE {
        return Err("the file is larger than 5 MB".into());
    }
    let bytes = fs::read(&path).map_err(|e| e.to_string())?;
    store(&bytes, dir, id)
}

/// Saves image bytes as `<id>.<stamp>.<ext>`. Older icons for `id` are left
/// for the caller to remove once the launcher points at the new one.
/// A fresh file name each time stops desktops from showing a cached old icon.
fn store(bytes: &[u8], dir: &Path, id: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());

    let (data, ext) = if looks_like_svg(bytes) {
        (bytes.to_vec(), "svg")
    } else {
        let img = decode_bounded(bytes)?;
        if img.width() < 16 || img.height() < 16 {
            return Err("image is too small".into());
        }
        let img = if img.width() > MAX_SIDE || img.height() > MAX_SIDE {
            img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Lanczos3)
        } else {
            img
        };
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        (out.into_inner(), "png")
    };

    let path = dir.join(format!("{id}.{stamp}.{ext}"));
    fs::write(&path, data).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Decodes a raster image without letting it allocate more than
/// `MAX_DECODE_ALLOC` or exceed `MAX_DECODE_SIDE` pixels per side.
fn decode_bounded(bytes: &[u8]) -> Result<image::DynamicImage, String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    reader.decode().map_err(|e| e.to_string())
}

/// Deletes every stored icon belonging to `id` except `keep` (ids never contain dots).
/// Compares file names only: `keep` may spell the same folder differently (symlinks).
pub fn remove_icons(dir: &Path, id: &str, keep: &Path) {
    let prefix = format!("{id}.");
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with(&prefix)
                && Some(e.file_name().as_os_str()) != keep.file_name()
            {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
    let head = head.trim_start_matches('\u{feff}').trim_start();
    head.starts_with("<svg") || (head.starts_with("<?xml") && head.contains("<svg"))
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .user_agent("webapp-manager/0.1")
        .build()
        .into()
}

fn get(agent: &ureq::Agent, url: &str, limit: u64) -> Option<Vec<u8>> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return None;
    }
    let mut resp = agent.get(url).call().ok()?;
    resp.body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .ok()
}

/// `href`s of `<link rel="...icon...">` tags, best first: apple-touch icons,
/// then by the largest declared size.
pub(crate) fn icon_links(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut found: Vec<(u32, String)> = Vec::new();
    let mut pos = 0;
    while let Some(start) = lower[pos..].find("<link") {
        let start = pos + start;
        let end = lower[start..].find('>').map_or(lower.len(), |e| start + e);
        let tag = &html[start..end];
        pos = end;
        let Some(rel) = attr(tag, "rel") else {
            continue;
        };
        let rel = rel.to_ascii_lowercase();
        if !rel
            .split_whitespace()
            .any(|r| r == "icon" || r == "apple-touch-icon" || r == "apple-touch-icon-precomposed")
        {
            continue;
        }
        let Some(href) = attr(tag, "href").filter(|h| !h.is_empty() && !h.starts_with("data:"))
        else {
            continue;
        };
        let size = attr(tag, "sizes")
            .and_then(|s| {
                s.split(['x', 'X'])
                    .next()
                    .and_then(|n| n.trim().parse().ok())
            })
            .unwrap_or(if rel.contains("apple-touch-icon") {
                180
            } else {
                32
            });
        // SVGs scale to any size, so they beat everything; then apple-touch icons.
        let is_svg = attr(tag, "type").is_some_and(|t| t.contains("svg"))
            || href
                .split(['?', '#'])
                .next()
                .is_some_and(|p| p.to_ascii_lowercase().ends_with(".svg"));
        let bonus = if is_svg {
            20_000
        } else if rel.contains("apple-touch-icon") {
            10_000
        } else {
            0
        };
        found.push((size + bonus, decode_entities(&href)));
    }
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found.into_iter().map(|(_, h)| h).collect()
}

/// Value of attribute `name` in an HTML tag, quoted or not.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let i = from + i;
        from = i + name.len();
        let before_ok = i == 0 || lower.as_bytes()[i - 1].is_ascii_whitespace();
        let rest = lower[from..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let vstart = tag.len() - rest.len() + 1;
        let v = tag[vstart..].trim_start();
        return Some(match v.chars().next() {
            Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or("").to_string(),
            _ => v
                .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
                .next()
                .unwrap_or("")
                .to_string(),
        });
    }
    None
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&#x2F;", "/")
        .replace("&#47;", "/")
}

/// `scheme://host[:port]` of `url`.
fn origin_of(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("https", url));
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    format!("{scheme}://{authority}")
}

/// Resolves `href` against the page URL.
pub(crate) fn resolve(base: &str, href: &str) -> String {
    if href.contains("://") {
        return href.to_string();
    }
    let scheme = base.split_once("://").map_or("https", |(s, _)| s);
    if let Some(rest) = href.strip_prefix("//") {
        return format!("{scheme}://{rest}");
    }
    let origin = origin_of(base);
    if href.starts_with('/') {
        return format!("{origin}{href}");
    }
    let path = base
        .get(origin.len()..)
        .unwrap_or("")
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
    format!("{origin}{dir}/{href}")
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_best_icon_link() {
        let html = r#"<head>
            <link rel="stylesheet" href="/a.css">
            <link rel="icon" type="image/png" sizes="16x16" href="/16.png">
            <LINK REL="shortcut icon" href='/fav.ico'>
            <link sizes="192x192" rel="icon" href="/192.png?v=1&amp;x=2"/>
            <link rel="apple-touch-icon" href="touch.png">
            <link rel="icon" href="data:image/png;base64,AAAA">
            <link rel="icon" type="image/svg+xml" href="/v.svg">
        </head>"#;
        assert_eq!(
            icon_links(html),
            vec![
                "/v.svg",
                "touch.png",
                "/192.png?v=1&x=2",
                "/fav.ico",
                "/16.png"
            ]
        );
    }

    #[test]
    fn resolves_relative_urls() {
        let base = "https://ex.com:8443/app/page?x=1";
        assert_eq!(resolve(base, "/i.png"), "https://ex.com:8443/i.png");
        assert_eq!(resolve(base, "i.png"), "https://ex.com:8443/app/i.png");
        assert_eq!(
            resolve(base, "//cdn.ex.com/i.png"),
            "https://cdn.ex.com/i.png"
        );
        assert_eq!(resolve("https://ex.com", "i.png"), "https://ex.com/i.png");
        assert_eq!(resolve(base, "https://o.com/i.png"), "https://o.com/i.png");
    }

    #[test]
    fn stores_png_and_svg_and_replaces_old() {
        let dir = tempfile::tempdir().unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(512, 300)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let p1 = store(png.get_ref(), dir.path(), "mail").unwrap();
        let img = image::open(&p1).unwrap();
        assert_eq!((img.width(), img.height()), (256, 150));

        std::thread::sleep(Duration::from_millis(2));
        let p2 = store(
            b"<?xml version='1.0'?>\n<svg xmlns='x'/>",
            dir.path(),
            "mail",
        )
        .unwrap();
        assert_eq!(p2.extension().unwrap(), "svg");
        remove_icons(dir.path(), "mail", &p2);
        assert!(!p1.exists() && p2.exists());

        // A different id with a shared prefix is left alone.
        let other = store(b"<svg/>", dir.path(), "mail-2").unwrap();
        remove_icons(dir.path(), "mail", Path::new(""));
        assert!(!p2.exists() && other.exists());

        assert!(store(b"not an image", dir.path(), "x").is_err());
    }

    #[test]
    fn rejects_images_claiming_huge_dimensions() {
        // A valid PNG header declaring 100000 x 100000 pixels.
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(1, 1)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let mut bytes = png.into_inner();
        bytes[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&100_000u32.to_be_bytes());
        // Re-sign the IHDR chunk (type + data = bytes 12..29) so only the size is "wrong".
        let crc = crc32(&bytes[12..29]);
        bytes[29..33].copy_from_slice(&crc.to_be_bytes());
        let err = decode_bounded(&bytes).unwrap_err();
        assert!(err.to_lowercase().contains("limit"), "{err}");
    }

    /// CRC-32 as PNG uses it (polynomial 0xEDB88320).
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn resolve_without_scheme_does_not_panic() {
        assert_eq!(resolve("example.com", "i.png"), "https://example.com/i.png");
    }

    #[test]
    fn imports_file_urls() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("my icon.svg");
        fs::write(&src, "<svg/>").unwrap();
        let url = format!("file://{}", src.to_string_lossy().replace(' ', "%20"));
        let out = import_file(Path::new(&url), &dir.path().join("icons"), "a").unwrap();
        assert!(out.exists());
    }
}
