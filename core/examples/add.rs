//! Adds a web app from the command line, using the real launcher folder.
//! `cargo run -p webapp-core --example add -- "<name>" <url> [browser-id]`
fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(name), Some(url)) = (args.next(), args.next()) else {
        eprintln!("usage: add <name> <url> [browser-id]");
        std::process::exit(2);
    };
    let input = webapp_core::WebAppInput {
        name,
        url,
        browser: args.next().unwrap_or_else(|| "chrome".into()),
        icon_file: String::new(),
    };
    match webapp_core::Store::user_default().save(None, &input) {
        Ok(app) => println!(
            "added {} ({}) icon: {}",
            app.name,
            app.url,
            if app.icon.is_empty() {
                "none found"
            } else {
                &app.icon
            }
        ),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1)
        }
    }
}
