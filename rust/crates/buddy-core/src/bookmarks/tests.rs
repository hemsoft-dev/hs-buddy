use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::client::{BackendError, classify, parse_categories, parse_list};
use super::title::{
    Resolver, TitleError, check_addresses, decode_html_entities, extract_page_title,
    fetch_page_title_with,
};
use super::url::{UrlError, is_internal_hostname, is_private_ip, validate_url};
use super::*;

fn bookmark(id: &str, category: &str, order: f64) -> Bookmark {
    Bookmark {
        id: id.into(),
        url: format!("https://example.com/{id}"),
        title: format!("Title {id}"),
        description: None,
        favicon_url: None,
        category: category.into(),
        tags: vec![],
        sort_order: order,
        last_visited_at: None,
        created_at: 1.0,
        updated_at: 1.0,
    }
}

#[test]
fn urls_follow_electrons_rules() {
    let cases: &[(&str, Result<(), UrlError>)] = &[
        ("https://example.com", Ok(())),
        ("http://example.com:8080/path?q=1#frag", Ok(())),
        ("https://sub.example.co.uk", Ok(())),
        ("https://8.8.8.8/", Ok(())),
        ("https://172.32.0.1/", Ok(())),
        ("https://172.15.0.1/", Ok(())),
        ("example.com", Err(UrlError::Invalid)),
        ("", Err(UrlError::Invalid)),
        ("http://", Err(UrlError::Invalid)),
        ("javascript:alert(1)", Err(UrlError::Protocol)),
        ("file:///etc/passwd", Err(UrlError::Protocol)),
        ("ftp://example.com", Err(UrlError::Protocol)),
        ("data:text/html,hi", Err(UrlError::Protocol)),
        ("buddy://settings", Err(UrlError::Protocol)),
        ("http://localhost:3210", Err(UrlError::Internal)),
        ("http://LOCALHOST/", Err(UrlError::Internal)),
        ("http://127.0.0.1/", Err(UrlError::Internal)),
        ("http://127.1.2.3/", Err(UrlError::Internal)),
        ("http://[::1]/", Err(UrlError::Internal)),
        ("http://10.0.0.5/", Err(UrlError::Internal)),
        ("http://192.168.1.1/", Err(UrlError::Internal)),
        (
            "http://169.254.169.254/latest/meta-data",
            Err(UrlError::Internal),
        ),
        ("http://172.16.0.1/", Err(UrlError::Internal)),
        ("http://172.31.255.255/", Err(UrlError::Internal)),
        ("http://printer.local/", Err(UrlError::Internal)),
        ("http://service.internal/", Err(UrlError::Internal)),
        // Stricter than Electron: IP literals are checked by range.
        ("http://0.0.0.0/", Err(UrlError::Internal)),
        ("http://[::]/", Err(UrlError::Internal)),
        ("http://[fe80::1]/", Err(UrlError::Internal)),
        ("http://[fd00::1]/", Err(UrlError::Internal)),
        ("http://[::ffff:127.0.0.1]/", Err(UrlError::Internal)),
        // The WHATWG parser normalizes numeric hosts before the check.
        ("http://2130706433/", Err(UrlError::Internal)),
        ("http://0x7f.1/", Err(UrlError::Internal)),
    ];
    for (url, expected) in cases {
        assert_eq!(validate_url(url).map(|_| ()), *expected, "{url}");
    }
}

#[test]
fn private_addresses_match_electrons_ranges() {
    for address in [
        "127.0.0.1",
        "0.0.0.0",
        "10.1.2.3",
        "172.20.0.1",
        "192.168.0.1",
        "169.254.1.1",
        "::1",
        "::",
        "fe80::1",
        "fc00::1",
        "fdff::1",
        "::ffff:10.0.0.1",
    ] {
        assert!(is_private_ip(address.parse().unwrap()), "{address}");
    }
    for address in ["8.8.8.8", "172.32.0.1", "2606:4700::1111", "::ffff:8.8.8.8"] {
        assert!(!is_private_ip(address.parse().unwrap()), "{address}");
    }
    assert!(is_internal_hostname("10.example.com"));
    assert!(!is_internal_hostname("example.com"));
    assert!(!is_internal_hostname("172.1.0.1"));
}

#[test]
fn a_name_with_any_private_address_is_refused() {
    let public: SocketAddr = "93.184.216.34:443".parse().unwrap();
    let private: SocketAddr = "10.0.0.1:443".parse().unwrap();
    assert_eq!(check_addresses(&[public]), Ok(()));
    assert_eq!(
        check_addresses(&[public, private]),
        Err(TitleError::Url(UrlError::Internal))
    );
    assert_eq!(check_addresses(&[]), Err(TitleError::Dns));
}

#[test]
fn page_titles_are_extracted_and_decoded() {
    assert_eq!(
        extract_page_title("<html><head><TITLE lang=en>\n  Rust &amp; GPUI\n  Guide </title>")
            .as_deref(),
        Some("Rust & GPUI Guide")
    );
    assert_eq!(
        extract_page_title(r#"<meta property="og:title" content="Open &quot;Graph&quot;">"#)
            .as_deref(),
        Some("Open \"Graph\"")
    );
    assert_eq!(
        extract_page_title(r#"<title> </title><meta content='Reversed' property='og:title'>"#)
            .as_deref(),
        Some("Reversed")
    );
    assert_eq!(
        extract_page_title(r#"<meta name="og:title" content="Not a property">"#),
        None
    );
    assert_eq!(extract_page_title("<p>No title</p>"), None);
    assert_eq!(extract_page_title("<title></title>"), None);
    assert_eq!(
        decode_html_entities(
            "&lt;a&gt; &#65;&#x42;&#X43; &nbsp;&apos;&bogus; &#0; &#xD800; &#99999999;"
        ),
        "<a> ABC  '&bogus; \u{FFFD} \u{FFFD} \u{FFFD}"
    );
    assert_eq!(decode_html_entities("Tom & Jerry"), "Tom & Jerry");
}

/// A loopback server answering each path with a canned response.
async fn serve(routes: Vec<(&'static str, String)>) -> u16 {
    serve_bytes(
        routes
            .into_iter()
            .map(|(path, response)| (path, response.into_bytes()))
            .collect(),
    )
    .await
}

async fn serve_bytes(routes: Vec<(&'static str, Vec<u8>)>) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let routes = Arc::new(routes);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let routes = routes.clone();
            tokio::spawn(async move {
                let mut request = vec![0; 4096];
                let read = stream.read(&mut request).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..read]).to_string();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                let response = routes
                    .iter()
                    .find(|(route, _)| *route == path)
                    .map(|(_, response)| response.clone())
                    .unwrap_or_else(|| {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec()
                    });
                let _ = stream.write_all(&response).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    port
}

fn html(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn redirect(location: &str) -> String {
    format!(
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
}

/// `fixture.test` resolves to the loopback server; the production resolver would refuse it.
fn fixture_resolver(port: u16) -> Resolver {
    Arc::new(move |host, _| {
        Box::pin(async move {
            if host == "fixture.test" {
                Ok(vec![SocketAddr::new(IpAddr::from([127, 0, 0, 1]), port)])
            } else {
                Err(TitleError::Dns)
            }
        })
    })
}

#[tokio::test]
async fn the_title_fetch_follows_checked_redirects_and_caps_the_body() {
    let large = format!("<title>Capped</title>{}", "x".repeat(200 * 1024));
    let port = serve(vec![
        ("/page", html("<title>Fixture page</title>")),
        ("/moved", redirect("/page")),
        ("/internal", redirect("http://127.0.0.1/admin")),
        ("/file", redirect("file:///etc/passwd")),
        ("/loop", redirect("/loop")),
        (
            "/json",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}"
                .into(),
        ),
        ("/large", html(&large)),
        ("/untitled", html("<p>nothing</p>")),
    ])
    .await;
    let resolver = fixture_resolver(port);
    let url = |path: &str| format!("http://fixture.test:{port}{path}");
    let fetch = |path: &str| {
        let (url, resolver) = (url(path), resolver.clone());
        async move { fetch_page_title_with(&url, &resolver).await }
    };
    assert_eq!(fetch("/page").await.as_deref(), Ok("Fixture page"));
    assert_eq!(fetch("/moved").await.as_deref(), Ok("Fixture page"));
    assert_eq!(fetch("/large").await.as_deref(), Ok("Capped"));
    assert_eq!(
        fetch("/internal").await,
        Err(TitleError::Url(UrlError::Internal))
    );
    assert_eq!(
        fetch("/file").await,
        Err(TitleError::Url(UrlError::Protocol))
    );
    assert_eq!(fetch("/loop").await, Err(TitleError::TooManyRedirects));
    assert_eq!(fetch("/json").await, Err(TitleError::NotHtml));
    assert_eq!(fetch("/missing").await, Err(TitleError::Status(404)));
    assert_eq!(fetch("/untitled").await, Err(TitleError::NoTitle));
    // Refused before any connection.
    assert_eq!(
        fetch_page_title_with(&format!("http://127.0.0.1:{port}/page"), &resolver).await,
        Err(TitleError::Url(UrlError::Internal))
    );
    assert_eq!(
        fetch_page_title_with("http://unknown.test/", &resolver).await,
        Err(TitleError::Dns)
    );
}

#[tokio::test]
async fn the_public_resolver_refuses_loopback_names() {
    let resolver = super::title::public_resolver();
    // `localhost` is refused by name before resolution; resolving it directly must
    // also be refused by address.
    assert_eq!(
        resolver("localhost".into(), 80).await,
        Err(TitleError::Url(UrlError::Internal))
    );
}

#[test]
fn list_results_are_checked_strictly() {
    let record = json!({
        "_id": "b1", "_creationTime": 1.0, "url": "https://example.com", "title": "Example",
        "category": "Dev/Rust", "tags": ["rust"], "sortOrder": 0.0,
        "createdAt": 1.0, "updatedAt": 2.0, "lastVisitedAt": 3.0, "description": "d",
    });
    let parsed = parse_list(json!([record.clone()])).unwrap();
    assert_eq!(parsed[0].id, "b1");
    assert_eq!(parsed[0].tags, ["rust"]);
    assert_eq!(parsed[0].last_visited_at, Some(3.0));
    assert_eq!(parsed[0].category_leaf(), "Rust");

    let mut minimal = record.clone();
    for key in ["tags", "description", "lastVisitedAt"] {
        minimal.as_object_mut().unwrap().remove(key);
    }
    assert!(parse_list(json!([minimal])).unwrap()[0].tags.is_empty());

    let broken = |key: &str, value: serde_json::Value| {
        let mut copy = record.clone();
        copy[key] = value;
        parse_list(json!([copy]))
    };
    assert_eq!(broken("_id", json!("")), Err(BackendError::InvalidSnapshot));
    assert_eq!(broken("url", json!(5)), Err(BackendError::InvalidSnapshot));
    assert_eq!(
        broken("tags", json!([1])),
        Err(BackendError::InvalidSnapshot)
    );
    assert_eq!(
        broken("sortOrder", json!("1")),
        Err(BackendError::InvalidSnapshot)
    );
    assert_eq!(
        broken("category", json!(null)),
        Err(BackendError::InvalidSnapshot)
    );
    assert_eq!(
        parse_list(json!([record.clone(), record])),
        Err(BackendError::InvalidSnapshot),
        "duplicate ids"
    );
    assert_eq!(parse_list(json!({})), Err(BackendError::InvalidSnapshot));
    assert_eq!(parse_categories(json!(["A", "B/C"])).unwrap(), ["A", "B/C"]);
    assert_eq!(
        parse_categories(json!(["A", 1])),
        Err(BackendError::InvalidSnapshot)
    );
}

#[test]
fn backend_messages_are_classified_without_their_text() {
    assert_eq!(
        classify("Uncaught Error: URL is already bookmarked in this category: https://x"),
        BackendError::Duplicate
    );
    assert_eq!(classify("Bookmark abc not found"), BackendError::Missing);
    assert_eq!(
        classify("ArgumentValidationError: Value does not match validator"),
        BackendError::Rejected
    );
    assert!(!BackendError::Duplicate.to_string().contains("https://"));
}

#[test]
fn create_omits_empty_fields_and_update_clears_them() {
    let draft = Form {
        url: " https://example.com ".into(),
        title: " Example ".into(),
        description: "  ".into(),
        category: " Dev ".into(),
        tags: " , ".into(),
    }
    .validate()
    .unwrap();
    assert_eq!(
        draft.arguments(),
        json!({"url": "https://example.com", "title": "Example", "category": "Dev"})
    );
    assert_eq!(
        draft.update_arguments(),
        json!({"url": "https://example.com", "title": "Example", "category": "Dev",
            "description": "", "tags": []})
    );
}

#[test]
fn the_editor_checks_fields_in_electrons_order() {
    let valid = Form {
        url: "https://example.com".into(),
        title: "Example".into(),
        description: " A page ".into(),
        category: "Dev/Frontend".into(),
        tags: "rust, gpui ,, ui".into(),
    };
    let draft = valid.validate().unwrap();
    assert_eq!(draft.tags, ["rust", "gpui", "ui"]);
    assert_eq!(draft.description.as_deref(), Some("A page"));
    let error = |change: fn(&mut Form)| {
        let mut form = valid.clone();
        change(&mut form);
        form.validate().unwrap_err()
    };
    assert_eq!(error(|form| form.url = " ".into()), "URL is required");
    assert_eq!(
        error(|form| {
            form.url.clear();
            form.title.clear();
        }),
        "URL is required"
    );
    assert_eq!(error(|form| form.title = "".into()), "Title is required");
    assert_eq!(
        error(|form| form.category = " ".into()),
        "Category is required"
    );
    assert_eq!(
        error(|form| form.url = "ftp://example.com".into()),
        "Only http and https URLs are allowed"
    );
    assert_eq!(
        error(|form| form.url = "http://192.168.0.1".into()),
        "Internal or private network URLs are not allowed"
    );
    assert_eq!(
        error(|form| form.url = "not a url".into()),
        "Please enter a valid URL (e.g., https://example.com)"
    );
    assert!(error(|form| form.category = "Dev/".into()).starts_with("Each part"));
    assert!(error(|form| form.category = "Dev//UI".into()).starts_with("Each part"));
    let mut many = valid.clone();
    many.tags = (0..MAX_TAGS)
        .map(|i| format!("t{i}"))
        .collect::<Vec<_>>()
        .join(",");
    assert!(many.validate().is_ok());
    many.tags.push_str(",one-more");
    assert_eq!(many.validate().unwrap_err(), "Maximum 50 tags allowed");
}

#[test]
fn filters_combine_category_tag_and_search() {
    let mut rust = bookmark("rust", "Dev/Rust", 1.0);
    rust.tags = vec!["lang".into(), "systems".into()];
    let mut gpui = bookmark("gpui", "Dev/Rust/UI", 0.0);
    gpui.description = Some("Zed's GPU UI framework".into());
    let mut devops = bookmark("devops", "Devops", 0.0);
    devops.tags = vec!["lang".into()];
    let mut news = bookmark("news", "News", 2.0);
    news.title = "Morning NEWS".into();
    let all = vec![rust, gpui, devops, news];
    let ids = |filter: Filter| -> Vec<String> {
        filter.apply(&all).iter().map(|b| b.id.clone()).collect()
    };
    assert_eq!(ids(Filter::default()), ["gpui", "devops", "rust", "news"]);
    // A category includes its subcategories but not a sibling sharing its prefix.
    let dev = Filter {
        category: "Dev".into(),
        ..Default::default()
    };
    assert_eq!(ids(dev.clone()), ["gpui", "rust"]);
    let tagged = Filter {
        tag: "lang".into(),
        ..dev.clone()
    };
    assert_eq!(ids(tagged), ["rust"]);
    let search = |text: &str| Filter {
        search: text.into(),
        ..Default::default()
    };
    assert_eq!(ids(search("news")), ["news"]);
    assert_eq!(ids(search("GPU ui")), ["gpui"]);
    assert_eq!(ids(search("SYSTEMS")), ["rust"]);
    assert_eq!(ids(search("example.com/devops")), ["devops"]);
    assert!(ids(search("nothing")).is_empty());
    assert!(!Filter::default().is_active());
    assert!(search("x").is_active());
    assert!(!search("  ").is_active());
    assert_eq!(all_tags(&all), ["lang", "systems"]);
}

#[test]
fn the_category_tree_nests_and_rolls_up_counts() {
    let all = vec![
        bookmark("a", "Dev", 0.0),
        bookmark("b", "Dev/Rust", 0.0),
        bookmark("c", "Dev/Rust", 1.0),
        bookmark("d", "Dev/Web/CSS", 0.0),
        bookmark("e", "News", 0.0),
    ];
    let tree = category_tree(&["Empty".into(), "Dev/Rust".into()], &all);
    let names: Vec<_> = tree.iter().map(|node| node.full_path.as_str()).collect();
    assert_eq!(names, ["Dev", "Empty", "News"]);
    let dev = &tree[0];
    assert_eq!((dev.direct_count, dev.total_count), (1, 4));
    let children: Vec<_> = dev
        .children
        .iter()
        .map(|node| node.full_path.as_str())
        .collect();
    assert_eq!(children, ["Dev/Rust", "Dev/Web"]);
    assert_eq!(
        (dev.children[0].direct_count, dev.children[0].total_count),
        (2, 2)
    );
    let web = &dev.children[1];
    assert_eq!((web.direct_count, web.total_count), (0, 1));
    assert_eq!(web.children[0].full_path, "Dev/Web/CSS");
    assert_eq!(tree[1].total_count, 0);
}

#[test]
fn drops_renumber_only_the_shared_category() {
    let all = vec![
        bookmark("a", "Dev", 0.0),
        bookmark("b", "Dev", 1.0),
        bookmark("c", "Dev", 2.0),
        bookmark("x", "News", 0.0),
    ];
    let order = |updates: Option<Vec<(String, f64)>>| -> Vec<String> {
        updates
            .unwrap()
            .into_iter()
            .enumerate()
            .map(|(index, (id, order))| {
                assert_eq!(order, index as f64);
                id
            })
            .collect()
    };
    assert_eq!(order(reorder(&all, "a", "c", Drop::Below)), ["b", "c", "a"]);
    assert_eq!(order(reorder(&all, "a", "c", Drop::Above)), ["b", "a", "c"]);
    assert_eq!(order(reorder(&all, "c", "a", Drop::Above)), ["c", "a", "b"]);
    assert_eq!(order(reorder(&all, "c", "a", Drop::Below)), ["a", "c", "b"]);
    // No change, the same row, or another category send nothing.
    assert_eq!(reorder(&all, "a", "b", Drop::Above), None);
    assert_eq!(reorder(&all, "a", "a", Drop::Below), None);
    assert_eq!(reorder(&all, "x", "a", Drop::Above), None);
    assert_eq!(reorder(&all, "missing", "a", Drop::Above), None);
    assert_eq!(order(shift(&all, "b", -1)), ["b", "a", "c"]);
    assert_eq!(order(shift(&all, "b", 1)), ["a", "c", "b"]);
    assert_eq!(shift(&all, "a", -1), None);
    assert_eq!(shift(&all, "c", 1), None);
    let mut local = all.clone();
    apply_order(&mut local, &reorder(&all, "a", "c", Drop::Below).unwrap());
    let ids: Vec<_> = in_order(&local, "Dev")
        .iter()
        .map(|b| b.id.clone())
        .collect();
    assert_eq!(ids, ["b", "c", "a"]);
}

#[test]
fn pasted_text_yields_one_http_url() {
    assert_eq!(
        pasted_url("  https://example.com/a  ").as_deref(),
        Some("https://example.com/a")
    );
    assert_eq!(
        pasted_url("# comment\nhttps://example.com/list\nhttps://second").as_deref(),
        Some("https://example.com/list")
    );
    assert_eq!(pasted_url("just some words"), None);
    assert_eq!(pasted_url("javascript:alert(1)"), None);
    assert_eq!(pasted_url(""), None);
}

#[test]
fn the_ai_prompt_matches_electron_and_replies_parse_safely() {
    let prompt = suggest::prompt("https://example.com", " Example ");
    assert!(prompt.starts_with("Given this bookmark URL and title, respond with ONLY a JSON object (no markdown, no code fences):\n{\"description\": "));
    assert!(prompt.contains("\n\nURL: https://example.com\nTitle: Example\n\nRules:\n- description: 1 short sentence, max 120 chars\n"));
    assert!(prompt.ends_with("- Respond with ONLY the JSON object, nothing else"));
    let untitled = suggest::prompt("https://example.com", "");
    assert!(untitled.starts_with("Given this bookmark URL, respond"));
    assert!(untitled.contains("URL: https://example.com\n\nRules:"));

    let parsed = suggest::parse(
        "```json\n{\"description\": \" A page. \", \"tags\": [\"web\", 3, \" \", \"a,b\", \"rust\"]}\n```",
    );
    assert_eq!(parsed.description.as_deref(), Some("A page."));
    assert_eq!(parsed.tags.unwrap(), ["web", "rust"]);
    assert_eq!(suggest::parse("not json"), suggest::Suggestion::default());
    assert_eq!(suggest::parse("[1, 2]"), suggest::Suggestion::default());
    let empty = suggest::parse("{\"description\": \"\", \"tags\": []}");
    assert_eq!(empty, suggest::Suggestion::default());
    let many = format!(
        "{{\"tags\": [{}]}}",
        (0..80)
            .map(|i| format!("\"t{i}\""))
            .collect::<Vec<_>>()
            .join(",")
    );
    assert_eq!(suggest::parse(&many).tags.unwrap().len(), MAX_TAGS);
}

/// A PNG's signature and IHDR, enough to be recognised and sized.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    bytes
}

fn image_response(content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

#[test]
fn favicon_formats_are_recognised_by_signature_and_sized_from_their_headers() {
    use super::favicon::{Format, dimensions, sniff};
    assert_eq!(sniff(&png(16, 16)), Some(Format::Png));
    assert_eq!(dimensions(Format::Png, &png(32, 48)), Some((32, 48)));
    let gif = b"GIF89a\x20\x00\x10\x00rest".to_vec();
    assert_eq!(sniff(&gif), Some(Format::Gif));
    assert_eq!(dimensions(Format::Gif, &gif), Some((32, 16)));
    // SOI, an APP0 segment, then a baseline frame of 64 x 40.
    let jpeg = [
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x28,
        0x00, 0x40, 0x03,
    ];
    assert_eq!(sniff(&jpeg), Some(Format::Jpeg));
    assert_eq!(dimensions(Format::Jpeg, &jpeg), Some((64, 40)));
    let mut bmp = b"BM".to_vec();
    bmp.resize(18, 0);
    bmp.extend_from_slice(&24i32.to_le_bytes());
    bmp.extend_from_slice(&(-24i32).to_le_bytes());
    assert_eq!(sniff(&bmp), Some(Format::Bmp));
    assert_eq!(dimensions(Format::Bmp, &bmp), Some((24, 24)));
    let mut webp = b"RIFF\0\0\0\0WEBPVP8X\0\0\0\0\0\0\0\0".to_vec();
    webp.extend_from_slice(&[31, 0, 0, 15, 0, 0]);
    assert_eq!(sniff(&webp), Some(Format::Webp));
    assert_eq!(dimensions(Format::Webp, &webp), Some((32, 16)));
    // An icon whose one entry is a 2048 x 2048 PNG, though the directory says 256.
    let mut ico = vec![0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 1, 0, 32, 0];
    ico.extend_from_slice(&(png(2048, 2048).len() as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend_from_slice(&png(2048, 2048));
    assert_eq!(sniff(&ico), Some(Format::Ico));
    assert_eq!(dimensions(Format::Ico, &ico), Some((2048, 2048)));
    for other in [
        b"<svg xmlns='http://www.w3.org/2000/svg'/>".as_slice(),
        b"<!doctype html>",
        b"",
        b"\x89PN",
    ] {
        assert_eq!(sniff(other), None);
    }
}

#[test]
fn favicon_images_must_be_small_and_readable() {
    use super::favicon::{FaviconError, Format, MAX_SIDE, check_image};
    assert_eq!(
        check_image(png(16, 16)).map(|icon| icon.format),
        Ok(Format::Png)
    );
    assert_eq!(
        check_image(png(MAX_SIDE, 1)).map(|icon| icon.format),
        Ok(Format::Png)
    );
    assert_eq!(
        check_image(png(MAX_SIDE + 1, 16)),
        Err(FaviconError::TooLarge)
    );
    assert_eq!(check_image(png(0, 16)), Err(FaviconError::NotAnImage));
    // Recognised, but too short to say its size.
    assert_eq!(
        check_image(b"\x89PNG\r\n\x1a\n".to_vec()),
        Err(FaviconError::NotAnImage)
    );
    assert_eq!(
        check_image(b"<svg width='99999'/>".to_vec()),
        Err(FaviconError::NotAnImage)
    );
}

#[test]
fn only_public_http_favicons_are_loaded_unless_the_fixture_serves_them() {
    use super::favicon::target;
    assert!(target("https://example.com/favicon.ico", None).is_some());
    assert!(target(" http://example.com/icon.png ", None).is_some());
    for refused in [
        "javascript:alert(1)",
        "data:image/png;base64,AAAA",
        "file:///etc/passwd",
        "ftp://example.com/icon.png",
        "http://localhost/icon.png",
        "http://127.0.0.1:8080/icon.png",
        "http://192.168.1.1/icon.png",
        "http://router.local/icon.png",
        "not a url",
        "",
    ] {
        assert!(target(refused, None).is_none(), "{refused}");
    }
    let fixture = crate::browser::FixtureOrigin::parse(Some("http://127.0.0.1:4321/"), true);
    let fixture = fixture.as_ref();
    assert!(target("http://127.0.0.1:4321/favicon.png", fixture).is_some());
    assert!(target("http://127.0.0.1:4322/favicon.png", fixture).is_none());
    assert!(target("http://127.0.0.2:4321/favicon.png", fixture).is_none());
}

#[tokio::test]
async fn the_favicon_fetch_checks_redirects_and_refuses_large_or_foreign_bodies() {
    use super::favicon::{FaviconError, Format, MAX_BYTES, fetch_favicon_with};
    let huge_declared = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        MAX_BYTES + 1
    )
    .into_bytes();
    // No length: the body itself runs past the limit.
    let mut huge_streamed =
        b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nConnection: close\r\n\r\n".to_vec();
    huge_streamed.extend_from_slice(&png(16, 16));
    huge_streamed.resize(huge_streamed.len() + MAX_BYTES, 0);
    let port = serve_bytes(vec![
        // A server that names the wrong type still serves an image.
        ("/icon.png", image_response("text/plain", &png(16, 16))),
        ("/moved", redirect("/icon.png").into_bytes()),
        (
            "/internal",
            redirect("http://127.0.0.1/icon.png").into_bytes(),
        ),
        ("/script", redirect("javascript:alert(1)").into_bytes()),
        ("/loop", redirect("/loop").into_bytes()),
        ("/huge-declared", huge_declared),
        ("/huge-streamed", huge_streamed),
        ("/page", html("<title>Not an icon</title>").into_bytes()),
        ("/large", image_response("image/png", &png(4096, 4096))),
    ])
    .await;
    let resolver = fixture_resolver(port);
    let fetch = |path: &str| {
        let (url, resolver) = (
            format!("http://fixture.test:{port}{path}"),
            resolver.clone(),
        );
        async move { fetch_favicon_with(&url, None, &resolver).await }
    };
    let icon = fetch("/icon.png").await.unwrap();
    assert_eq!((icon.format, icon.bytes), (Format::Png, png(16, 16)));
    assert_eq!(
        fetch("/moved").await.map(|icon| icon.format),
        Ok(Format::Png)
    );
    assert_eq!(
        fetch("/internal").await,
        Err(FaviconError::Url(UrlError::Internal))
    );
    assert_eq!(
        fetch("/script").await,
        Err(FaviconError::Url(UrlError::Protocol))
    );
    assert_eq!(fetch("/loop").await, Err(FaviconError::TooManyRedirects));
    assert_eq!(fetch("/huge-declared").await, Err(FaviconError::TooLarge));
    assert_eq!(fetch("/huge-streamed").await, Err(FaviconError::TooLarge));
    assert_eq!(fetch("/page").await, Err(FaviconError::NotAnImage));
    assert_eq!(fetch("/large").await, Err(FaviconError::TooLarge));
    assert_eq!(fetch("/missing").await, Err(FaviconError::Status(404)));
    // The loopback address itself is refused, unless it is the fixture's origin.
    let direct = format!("http://127.0.0.1:{port}/icon.png");
    assert_eq!(
        fetch_favicon_with(&direct, None, &resolver).await,
        Err(FaviconError::Url(UrlError::Internal))
    );
    let fixture =
        crate::browser::FixtureOrigin::parse(Some(&format!("http://127.0.0.1:{port}/")), true);
    assert_eq!(
        fetch_favicon_with(&direct, fixture.as_ref(), &resolver)
            .await
            .map(|icon| icon.format),
        Ok(Format::Png)
    );
}
