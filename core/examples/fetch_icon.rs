//! Manual check: `cargo run -p webapp-core --example fetch_icon -- <url> <out-dir>`
fn main() {
    let mut args = std::env::args().skip(1);
    let url = webapp_core::normalize_url(&args.next().expect("url")).unwrap();
    let dir = std::path::PathBuf::from(args.next().expect("out dir"));
    match webapp_core::icon::fetch_site_icon(&url, &dir, "test") {
        Some(p) => println!("{url} -> {}", p.display()),
        None => println!("{url} -> no icon"),
    }
}
