//! Dev helper: fills a scratch data dir with sample web apps.
//! `cargo run -p webapp-core --example seed -- <data-dir>`
fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("data dir"));
    let store = webapp_core::Store::new(dir.join("applications"), dir.join("webapp-manager/icons"));
    for (name, url) in [
        ("WhatsApp", "web.whatsapp.com"),
        ("GitHub", "github.com"),
        ("YouTube Music", "music.youtube.com"),
        ("Notion", "notion.so"),
        ("Team Calendar", "cal.invalid"),
    ] {
        let input = webapp_core::WebAppInput {
            name: name.into(),
            url: url.into(),
            browser: "chrome".into(),
            icon_file: String::new(),
        };
        match store.save(None, &input) {
            Ok(a) => println!(
                "{} -> {}",
                a.name,
                if a.icon.is_empty() { "no icon" } else { "icon" }
            ),
            Err(e) => println!("{name}: {e}"),
        }
    }
}
