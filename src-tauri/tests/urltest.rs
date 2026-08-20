// Test that bridg.json URL patterns match what we actually navigate to.
// Mirrors the same urlpattern API path tauri-utils uses internally
// (see tauri-utils-2.9.3/src/acl/mod.rs:280-299).

#[test]
fn test_bridge_pattern_matches_chat_url() {
    fn parse(s: &str) -> urlpattern::UrlPattern {
        let init = urlpattern::UrlPatternInit::parse_constructor_string::<regex::Regex>(s, None).unwrap();
        urlpattern::UrlPattern::parse(init, Default::default()).unwrap()
    }

    fn matches(s: &str, url: &str) -> bool {
        let pattern = parse(s);
        let url = url::Url::parse(url).unwrap();
        pattern.test(urlpattern::UrlPatternMatchInput::Url(url)).unwrap()
    }

    println!("\n=== pattern: http://127.0.0.1:28789/* ===");
    for url in &[
        "http://127.0.0.1:28789/",
        "http://127.0.0.1:28789/?v=1234567890",
        "http://127.0.0.1:28789/index.html",
        "http://127.0.0.1:28789",
        "http://localhost:28789/?v=1234567890",
    ] {
        println!("  {} → match={}", url, matches("http://127.0.0.1:28789/*", url));
    }

    println!("\n=== pattern: http://127.0.0.1:28789/?* ===");
    for url in &[
        "http://127.0.0.1:28789/",
        "http://127.0.0.1:28789/?v=1234567890",
        "http://127.0.0.1:28789/index.html",
    ] {
        println!("  {} → match={}", url, matches("http://127.0.0.1:28789/?*", url));
    }
}
