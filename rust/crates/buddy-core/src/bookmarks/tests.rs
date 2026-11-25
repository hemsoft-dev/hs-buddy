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

/// One PNG chunk: its length, type, data and the CRC-32 decoders check.
fn png_chunk(bytes: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
    bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = bytes.len();
    bytes.extend_from_slice(kind);
    bytes.extend_from_slice(data);
    let mut crc = !0u32;
    for byte in &bytes[start..] {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
    }
    bytes.extend_from_slice(&(!crc).to_be_bytes());
}

/// A PNG whose header claims `width` x `height`, followed by a stub of image
/// data: decoders read its size, and fail if asked for its pixels.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut header = width.to_be_bytes().to_vec();
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    png_chunk(&mut bytes, b"IHDR", &header);
    png_chunk(&mut bytes, b"IDAT", &[0x78, 0x9c]);
    png_chunk(&mut bytes, b"IEND", &[]);
    bytes
}

/// `png` with `chunks` inserted straight after its IHDR.
fn with_chunks(png: &[u8], chunks: &[(&[u8], Vec<u8>)]) -> Vec<u8> {
    // The signature, then IHDR: 4 + 4 + 13 + 4 bytes.
    let (head, tail) = png.split_at(8 + 25);
    let mut bytes = head.to_vec();
    for (kind, data) in chunks {
        png_chunk(&mut bytes, kind, data);
    }
    bytes.extend_from_slice(tail);
    bytes
}

/// The chunk types of a PNG, in order.
fn chunk_kinds(png: &[u8]) -> Vec<String> {
    let mut kinds = Vec::new();
    let mut at = 8;
    while at + 8 <= png.len() {
        let length = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
        kinds.push(String::from_utf8_lossy(&png[at + 4..at + 8]).into_owned());
        at += 12 + length;
    }
    kinds
}

/// A zlib stream that inflates to `1 + 258 * matches` zero bytes: one literal
/// zero, then that many length-258 copies of the byte before, in a single
/// fixed-Huffman block. 13 bits of input for every 258 bytes of output.
fn zlib_zeros(matches: usize) -> Vec<u8> {
    struct Bits {
        out: Vec<u8>,
        acc: u64,
        count: u32,
    }
    impl Bits {
        fn put(&mut self, value: u32, count: u32) {
            self.acc |= u64::from(value) << self.count;
            self.count += count;
            while self.count >= 8 {
                self.out.push(self.acc as u8);
                self.acc >>= 8;
                self.count -= 8;
            }
        }
        /// A Huffman code, which deflate stores from its top bit down.
        fn code(&mut self, code: u32, count: u32) {
            self.put(code.reverse_bits() >> (32 - count), count);
        }
    }
    let mut bits = Bits {
        out: vec![0x78, 0x01],
        acc: 0,
        count: 0,
    };
    // The last block, fixed Huffman codes.
    bits.put(1, 1);
    bits.put(1, 2);
    // Literal 0, then length 258 (symbol 285) at distance 1 (code 0).
    bits.code(0x30, 8);
    for _ in 0..matches {
        bits.code(0xC5, 8);
        bits.put(0, 5);
    }
    // End of block.
    bits.code(0, 7);
    if bits.count > 0 {
        bits.put(0, 8 - bits.count);
    }
    let length = 1 + 258 * matches as u64;
    // Adler-32 of zeros: a stays 1 and b adds 1 for each byte.
    let adler = ((length % 65_521) << 16) | 1;
    bits.out.extend_from_slice(&(adler as u32).to_be_bytes());
    bits.out
}

/// An iCCP chunk's data: a profile name, compression 0 and `profile`.
fn iccp(profile: Vec<u8>) -> Vec<u8> {
    let mut data = b"bomb\0\0".to_vec();
    data.extend(profile);
    data
}

/// The test binary's allocator, counting what each thread has allocated, so a
/// test can see the most a call held at once.
mod counting {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    pub struct Counting;

    thread_local! {
        static LIVE: Cell<usize> = const { Cell::new(0) };
        static PEAK: Cell<usize> = const { Cell::new(0) };
    }

    fn track(grown: usize, shrunk: usize) {
        let _ = LIVE.try_with(|live| {
            let now = live.get().saturating_add(grown).saturating_sub(shrunk);
            live.set(now);
            let _ = PEAK.try_with(|peak| peak.set(peak.get().max(now)));
        });
    }

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let pointer = unsafe { System.alloc(layout) };
            if !pointer.is_null() {
                track(layout.size(), 0);
            }
            pointer
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let pointer = unsafe { System.alloc_zeroed(layout) };
            if !pointer.is_null() {
                track(layout.size(), 0);
            }
            pointer
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            unsafe { System.dealloc(pointer, layout) };
            track(0, layout.size());
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            let moved = unsafe { System.realloc(pointer, layout, size) };
            if !moved.is_null() {
                track(size, layout.size());
            }
            moved
        }
    }

    /// `f`'s result and the most it held allocated at once on this thread.
    pub fn peak<T>(f: impl FnOnce() -> T) -> (T, usize) {
        let base = LIVE.with(Cell::get);
        PEAK.with(|peak| peak.set(base));
        let result = f();
        (result, PEAK.with(Cell::get).saturating_sub(base))
    }
}

#[global_allocator]
static ALLOCATOR: counting::Counting = counting::Counting;

fn image_response(content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

/// A real image of one colour, encoded in `format`.
fn encoded(format: image::ImageFormat, width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(width, height, image::Rgba(rgba));
    let mut bytes = std::io::Cursor::new(Vec::new());
    match format {
        // JPEG and BMP here take no alpha.
        image::ImageFormat::Jpeg | image::ImageFormat::Bmp => image::DynamicImage::from(image)
            .into_rgb8()
            .write_to(&mut bytes, format)
            .unwrap(),
        _ => image.write_to(&mut bytes, format).unwrap(),
    }
    bytes.into_inner()
}

/// An icon directory with one 32-bit entry per `(side, image)`; a side of 0
/// declares 256 px, as the format does.
fn icon(entries: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut bytes = vec![0, 0, 1, 0];
    bytes.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let mut offset = 6 + entries.len() * 16;
    for (side, image) in entries {
        bytes.extend_from_slice(&[*side, *side, 0, 0, 1, 0, 32, 0]);
        bytes.extend_from_slice(&(image.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += image.len();
    }
    for (_, image) in entries {
        bytes.extend_from_slice(image);
    }
    bytes
}

#[test]
fn favicon_formats_are_recognised_by_their_signature() {
    use super::favicon::{Format, sniff};
    use image::ImageFormat;
    for (format, expected) in [
        (ImageFormat::Png, Format::Png),
        (ImageFormat::Jpeg, Format::Jpeg),
        (ImageFormat::Gif, Format::Gif),
        (ImageFormat::Ico, Format::Ico),
        (ImageFormat::Bmp, Format::Bmp),
    ] {
        assert_eq!(
            sniff(&encoded(format, 4, 4, [1, 2, 3, 255])),
            Some(expected)
        );
    }
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
fn favicons_are_decoded_here_to_one_small_frame() {
    use super::favicon::{Format, ICON_SIDE, MAX_SIDE, sanitize};
    use image::ImageFormat;
    for (format, expected) in [
        (ImageFormat::Png, Format::Png),
        (ImageFormat::Jpeg, Format::Jpeg),
        (ImageFormat::Gif, Format::Gif),
        (ImageFormat::Ico, Format::Ico),
        (ImageFormat::Bmp, Format::Bmp),
    ] {
        let icon = sanitize(&encoded(format, 32, 32, [255, 0, 255, 255])).unwrap();
        assert_eq!(icon.format, expected, "{format:?}");
        assert_eq!(icon.image.dimensions(), (32, 32), "{format:?}");
        let pixel = icon.image.get_pixel(16, 16).0;
        assert!(
            pixel[0] > 220 && pixel[1] < 40 && pixel[2] > 220,
            "{format:?} {pixel:?}"
        );
    }
    // Every result fits ICON_SIDE a side, keeping its shape.
    for (width, height, fitted) in [
        (16, 16, (16, 16)),
        (64, 64, (64, 64)),
        (300, 100, (64, 21)),
        (100, 300, (21, 64)),
        (MAX_SIDE, MAX_SIDE, (64, 64)),
        (MAX_SIDE, 1, (64, 1)),
    ] {
        let icon = sanitize(&encoded(ImageFormat::Png, width, height, [9, 9, 9, 255])).unwrap();
        assert_eq!(icon.image.dimensions(), fitted, "{width} x {height}");
        assert!(icon.image.width() <= ICON_SIDE && icon.image.height() <= ICON_SIDE);
    }
}

/// A WebP whose canvas is `canvas` px a side and whose lossy VP8 frame
/// declares `frame` px a side, with no pixel data after the frame header.
fn webp_with_frame(canvas: u32, frame: u16) -> Vec<u8> {
    let mut vp8x = b"VP8X".to_vec();
    vp8x.extend_from_slice(&10u32.to_le_bytes());
    vp8x.extend_from_slice(&[0; 4]);
    vp8x.extend_from_slice(&(canvas - 1).to_le_bytes()[..3]);
    vp8x.extend_from_slice(&(canvas - 1).to_le_bytes()[..3]);
    // A key frame tag (version 0, shown, a 16 byte first partition), the
    // start code, then the frame's width and height.
    let tag: u32 = (1 << 4) | (16 << 5);
    let mut frame_data = tag.to_le_bytes()[..3].to_vec();
    frame_data.extend_from_slice(&[0x9d, 0x01, 0x2a]);
    frame_data.extend_from_slice(&frame.to_le_bytes());
    frame_data.extend_from_slice(&frame.to_le_bytes());
    frame_data.extend_from_slice(&[0; 22]);
    let mut vp8 = b"VP8 ".to_vec();
    vp8.extend_from_slice(&(frame_data.len() as u32).to_le_bytes());
    vp8.extend_from_slice(&frame_data);
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&((4 + vp8x.len() + vp8.len()) as u32).to_le_bytes());
    bytes.extend_from_slice(b"WEBP");
    bytes.extend_from_slice(&vp8x);
    bytes.extend_from_slice(&vp8);
    bytes
}

#[test]
fn a_webp_favicon_is_refused_without_being_decoded() {
    use super::favicon::{FaviconError, MAX_BYTES, sanitize, sniff};
    use image::ImageFormat;
    // A 1 px canvas holding a 2048 px lossy frame: image-webp 0.2 decodes the
    // frame, 6 MiB of planes, before it compares it with the canvas.
    let oversized = webp_with_frame(1, 2048);
    assert!(oversized.len() < MAX_BYTES);
    // A sound WebP is refused the same way, so the refusal is the signature's,
    // not a decode that failed.
    let sound = encoded(ImageFormat::WebP, 16, 16, [255, 0, 255, 255]);
    assert!(image::load_from_memory(&sound).is_ok());
    for bytes in [oversized, sound] {
        assert_eq!(sniff(&bytes), None);
        assert_eq!(sanitize(&bytes), Err(FaviconError::NotAnImage));
    }
}

#[test]
fn a_many_frame_gif_yields_only_its_first_frame() {
    use super::favicon::{ICON_SIDE, MAX_BYTES, sanitize};
    use image::codecs::gif::GifEncoder;
    let mut bytes = Vec::new();
    {
        let mut encoder = GifEncoder::new_with_speed(&mut bytes, 30);
        let frames = (0..200).map(|n| {
            let colour = if n == 0 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            };
            image::Frame::new(image::RgbaImage::from_pixel(256, 256, image::Rgba(colour)))
        });
        encoder.encode_frames(frames).unwrap();
    }
    // Small enough to be fetched, and 200 full frames once decoded.
    assert!(bytes.len() < MAX_BYTES, "{}", bytes.len());
    let icon = sanitize(&bytes).unwrap();
    assert_eq!(icon.image.dimensions(), (ICON_SIDE, ICON_SIDE));
    assert_eq!(icon.image.get_pixel(32, 32).0, [255, 0, 0, 255]);
}

/// A one-frame GIF on a `screen` px canvas whose `width` x `height` frame
/// starts one pixel in from the left, so it is decoded into its own buffer.
fn gif_off_canvas(screen: u16, width: u32, height: u32) -> Vec<u8> {
    let mut bytes = encoded(image::ImageFormat::Gif, width, height, [255, 0, 255, 255]);
    bytes[6..8].copy_from_slice(&screen.to_le_bytes());
    bytes[8..10].copy_from_slice(&screen.to_le_bytes());
    // The blocks start after the screen descriptor and its colour table.
    let mut at = 13;
    if bytes[10] & 0x80 != 0 {
        at += 3 << ((bytes[10] & 0x07) + 1);
    }
    while bytes[at] == 0x21 {
        at += 2;
        while bytes[at] != 0 {
            at += usize::from(bytes[at]) + 1;
        }
        at += 1;
    }
    assert_eq!(bytes[at], 0x2c, "an image descriptor");
    bytes[at + 1..at + 3].copy_from_slice(&1u16.to_le_bytes());
    bytes
}

#[test]
fn a_gifs_frame_and_palette_indices_stay_within_the_decode_limit() {
    use super::favicon::{
        FaviconError, MAX_BYTES, MAX_DECODE_BYTES, MAX_GIF_DECODE_BYTES, sanitize,
    };
    // A 16 px canvas and a frame just within what image may count for a GIF:
    // the frame's RGBA buffer, and the gif decoder's index byte per frame
    // pixel beside it, which image doesn't count.
    let canvas = 16 * 16 * 4;
    let near = gif_off_canvas(16, 886, 887);
    assert!(near.len() < MAX_BYTES, "{}", near.len());
    assert!(canvas + 886 * 887 * 4 <= MAX_GIF_DECODE_BYTES);
    let (decoded, peak) = counting::peak(|| sanitize(&near));
    let decoded = decoded.unwrap();
    assert_eq!(decoded.image.dimensions(), (16, 16));
    assert_eq!(decoded.image.get_pixel(0, 8).0, [0, 0, 0, 0]);
    assert_eq!(decoded.image.get_pixel(8, 8).0, [255, 0, 255, 255]);
    assert!(
        peak < MAX_DECODE_BYTES as usize,
        "{peak} bytes held at once"
    );
    // A frame image would count within the 4 MiB for any other format: its
    // indices would take the decode about 1 MiB past it, so it is refused.
    let over = gif_off_canvas(16, 1023, 1024);
    assert!(over.len() < MAX_BYTES, "{}", over.len());
    assert!(canvas + 1023 * 1024 * 4 <= MAX_DECODE_BYTES);
    let (refused, peak) = counting::peak(|| sanitize(&over));
    assert_eq!(refused, Err(FaviconError::TooLarge));
    assert!(
        peak < MAX_DECODE_BYTES as usize,
        "{peak} bytes held at once"
    );
}

#[test]
fn an_icon_is_decoded_within_the_limits_whatever_its_directory_says() {
    use super::favicon::{FaviconError, MAX_BYTES, MAX_ICON_ENTRIES, sanitize};
    use image::ImageFormat;
    let small = encoded(ImageFormat::Png, 16, 16, [0, 255, 0, 255]);
    let mut entries: Vec<(u8, Vec<u8>)> = (0..70).map(|_| (16, small.clone())).collect();
    // The 71st entry claims 256 px and holds a PNG of 60000 px a side: it is
    // passed over, unallocated, for the best entry that fits.
    entries.push((0, png(60_000, 60_000)));
    let oversized = icon(&entries);
    assert!(oversized.len() < MAX_BYTES);
    let picked = sanitize(&oversized).unwrap();
    assert_eq!(picked.image.dimensions(), (16, 16));
    assert_eq!(picked.image.get_pixel(8, 8).0, [0, 255, 0, 255]);
    // With nothing that fits, the icon is too large.
    assert_eq!(
        sanitize(&icon(&[(0, png(60_000, 60_000))])),
        Err(FaviconError::TooLarge)
    );
    // A sound entry that far down the directory is the best one.
    entries.pop();
    entries.push((32, encoded(ImageFormat::Png, 32, 32, [0, 0, 255, 255])));
    let icon_bytes = icon(&entries);
    let icon_image = sanitize(&icon_bytes).unwrap();
    assert_eq!(icon_image.image.dimensions(), (32, 32));
    assert_eq!(icon_image.image.get_pixel(8, 8).0, [0, 0, 255, 255]);
    // A directory with no entries, too many, or one past the end of the file.
    let mut empty = icon(&[(16, small.clone())]);
    empty[4] = 0;
    let too_many: Vec<(u8, Vec<u8>)> = (0..=MAX_ICON_ENTRIES)
        .map(|_| (16, small.clone()))
        .collect();
    let too_many = icon(&too_many);
    assert!(too_many.len() < MAX_BYTES);
    let mut past_the_end = icon(&[(16, small.clone())]);
    past_the_end[6 + 8] = past_the_end[6 + 8].wrapping_add(1);
    let mut cut_short = icon(&[(16, small.clone())]);
    cut_short.truncate(6 + 8);
    for refused in [empty, too_many, past_the_end, cut_short] {
        assert_eq!(sanitize(&refused), Err(FaviconError::NotAnImage));
    }
}

#[test]
fn an_icons_png_entry_is_decoded_without_its_compressed_metadata() {
    use super::favicon::{MAX_BYTES, MAX_DECODE_BYTES, filter_png, sanitize};
    use image::ImageFormat;
    // A 16 px PNG whose colour profile inflates to 16 MiB: the ICO decoder's
    // own PNG decoder reads it with no limits.
    let bomb = zlib_zeros(65_000);
    let png = with_chunks(
        &encoded(ImageFormat::Png, 16, 16, [255, 0, 255, 255]),
        &[(b"iCCP".as_slice(), iccp(bomb))],
    );
    let ico = icon(&[(16, png.clone())]);
    assert!(ico.len() < MAX_BYTES, "{}", ico.len());
    let filtered = filter_png(&png).unwrap();
    assert!(!chunk_kinds(&filtered).contains(&"iCCP".to_owned()));
    assert!(filtered.len() < 1024, "{}", filtered.len());
    let (decoded, peak) = counting::peak(|| sanitize(&ico));
    let decoded = decoded.unwrap();
    assert_eq!(decoded.image.dimensions(), (16, 16));
    assert_eq!(decoded.image.get_pixel(8, 8).0, [255, 0, 255, 255]);
    assert!(
        peak < MAX_DECODE_BYTES as usize,
        "{peak} bytes held at once"
    );
}

#[test]
fn an_icons_bitmap_entry_is_decoded_on_its_own() {
    use super::favicon::sanitize;
    use image::ImageFormat;
    // A 16 px, 32-bit DIB: BITMAPINFOHEADER with the height doubled for the
    // AND mask, bottom-up BGRA rows, then a mask that hides nothing.
    let mut dib = 40u32.to_le_bytes().to_vec();
    dib.extend_from_slice(&16i32.to_le_bytes());
    dib.extend_from_slice(&32i32.to_le_bytes());
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&32u16.to_le_bytes());
    dib.extend_from_slice(&[0; 24]);
    for _ in 0..16 * 16 {
        dib.extend_from_slice(&[255, 128, 0, 255]);
    }
    dib.extend_from_slice(&[0; 16 * 4]);
    // The bitmap has more pixels than the PNG beside it, so it is picked.
    let both = icon(&[
        (8, encoded(ImageFormat::Png, 8, 8, [255, 0, 255, 255])),
        (16, dib),
    ]);
    let icon_image = sanitize(&both).unwrap();
    assert_eq!(icon_image.image.dimensions(), (16, 16));
    assert_eq!(icon_image.image.get_pixel(3, 3).0, [0, 128, 255, 255]);
}

#[test]
fn a_png_keeps_only_the_chunks_a_favicon_needs() {
    use super::favicon::{MAX_BYTES, MAX_DECODE_BYTES, filter_png, sanitize};
    use image::ImageFormat;
    let mut text = b"Comment\0\0".to_vec();
    text.extend(zlib_zeros(65_000));
    let png = with_chunks(
        &encoded(ImageFormat::Png, 16, 16, [0, 255, 255, 255]),
        &[
            (b"iCCP".as_slice(), iccp(zlib_zeros(8))),
            (b"gAMA".as_slice(), 45_455u32.to_be_bytes().to_vec()),
            (b"sRGB".as_slice(), vec![0]),
            (b"tEXt".as_slice(), b"Title\0icon".to_vec()),
            (b"zTXt".as_slice(), text),
        ],
    );
    assert!(png.len() < MAX_BYTES);
    let kinds = chunk_kinds(&png);
    for kind in ["iCCP", "gAMA", "sRGB", "tEXt", "zTXt"] {
        assert!(kinds.contains(&kind.to_owned()), "{kind}");
    }
    let filtered = filter_png(&png).unwrap();
    let kinds = chunk_kinds(&filtered);
    assert_eq!(kinds.first().map(String::as_str), Some("IHDR"));
    assert_eq!(kinds.last().map(String::as_str), Some("IEND"));
    assert!(
        kinds
            .iter()
            .all(|kind| ["IHDR", "IDAT", "IEND"].contains(&kind.as_str())),
        "{kinds:?}"
    );
    let (decoded, peak) = counting::peak(|| sanitize(&png));
    assert_eq!(decoded.unwrap().image.get_pixel(4, 4).0, [0, 255, 255, 255]);
    assert!(
        peak < MAX_DECODE_BYTES as usize,
        "{peak} bytes held at once"
    );
    // A palette and its transparency are kept, so an indexed PNG still works.
    let mut indexed = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = 1u32.to_be_bytes().to_vec();
    header.extend_from_slice(&1u32.to_be_bytes());
    header.extend_from_slice(&[8, 3, 0, 0, 0]);
    png_chunk(&mut indexed, b"IHDR", &header);
    png_chunk(&mut indexed, b"PLTE", &[255, 0, 0]);
    png_chunk(&mut indexed, b"tRNS", &[128]);
    // One row: filter type 0, palette index 0, stored uncompressed.
    let row = [0u8, 0];
    let mut idat = vec![0x78, 0x01, 0x01, 2, 0, !2, 0xFF];
    idat.extend_from_slice(&row);
    // Adler-32 of two zero bytes: a stays 1, b is 1 + 1.
    let adler = (2u32 << 16) | 1;
    idat.extend_from_slice(&adler.to_be_bytes());
    png_chunk(&mut indexed, b"IDAT", &idat);
    png_chunk(&mut indexed, b"IEND", &[]);
    assert_eq!(
        chunk_kinds(&filter_png(&indexed).unwrap()),
        ["IHDR", "PLTE", "tRNS", "IDAT", "IEND"]
    );
    assert_eq!(
        sanitize(&indexed).unwrap().image.get_pixel(0, 0).0,
        [255, 0, 0, 128]
    );
}

#[test]
fn malformed_png_chunks_are_refused() {
    use super::favicon::{FaviconError, filter_png, sanitize};
    use image::ImageFormat;
    let sound = encoded(ImageFormat::Png, 4, 4, [1, 2, 3, 255]);
    assert!(filter_png(&sound).is_ok());
    let ihdr_end = 8 + 25;
    // A chunk length past the end of the file, or past what a PNG allows.
    let mut long = sound.clone();
    long[ihdr_end..ihdr_end + 4].copy_from_slice(&(sound.len() as u32).to_be_bytes());
    let mut huge = sound.clone();
    huge[ihdr_end..ihdr_end + 4].copy_from_slice(&u32::MAX.to_be_bytes());
    // A CRC that doesn't match.
    let mut bad_crc = sound.clone();
    bad_crc[ihdr_end - 1] ^= 1;
    // Cut off before IEND.
    let cut = sound[..sound.len() - 12].to_vec();
    // IHDR missing, repeated, or the wrong length.
    let mut no_header = b"\x89PNG\r\n\x1a\n".to_vec();
    no_header.extend_from_slice(&sound[ihdr_end..]);
    let twice = with_chunks(&sound, &[(b"IHDR".as_slice(), sound[16..29].to_vec())]);
    let mut short_header = b"\x89PNG\r\n\x1a\n".to_vec();
    png_chunk(&mut short_header, b"IHDR", &sound[16..28]);
    short_header.extend_from_slice(&sound[ihdr_end..]);
    // No image data.
    let mut no_data = sound[..ihdr_end].to_vec();
    png_chunk(&mut no_data, b"IEND", &[]);
    // A critical chunk a favicon can't do without knowing, and a chunk type
    // that isn't four letters.
    let critical = with_chunks(&sound, &[(b"CgBI".as_slice(), vec![0; 4])]);
    let not_letters = with_chunks(&sound, &[(b"a1b2".as_slice(), vec![])]);
    for (name, bytes) in [
        ("long", long),
        ("huge", huge),
        ("bad crc", bad_crc),
        ("cut", cut),
        ("no header", no_header),
        ("twice", twice),
        ("short header", short_header),
        ("no data", no_data),
        ("critical", critical),
        ("not letters", not_letters),
    ] {
        assert_eq!(filter_png(&bytes), Err(FaviconError::NotAnImage), "{name}");
        assert_eq!(sanitize(&bytes), Err(FaviconError::NotAnImage), "{name}");
    }
    // Whatever follows IEND is dropped.
    let mut trailing = sound.clone();
    trailing.extend_from_slice(b"trailing bytes");
    assert_eq!(filter_png(&trailing).unwrap(), filter_png(&sound).unwrap());
}

#[test]
fn a_bitmap_with_a_core_header_is_decoded() {
    use super::favicon::sanitize;
    // BITMAPFILEHEADER, then the 12 byte BITMAPCOREHEADER: 16-bit width and
    // height side by side, one plane, 24 bits a pixel; rows of 48 bytes.
    let mut bmp = b"BM".to_vec();
    bmp.extend_from_slice(&(26u32 + 16 * 48).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&26u32.to_le_bytes());
    bmp.extend_from_slice(&12u32.to_le_bytes());
    bmp.extend_from_slice(&16u16.to_le_bytes());
    bmp.extend_from_slice(&16u16.to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&24u16.to_le_bytes());
    for _ in 0..16 * 16 {
        // Blue, green, red.
        bmp.extend_from_slice(&[0, 128, 255]);
    }
    let icon = sanitize(&bmp).unwrap();
    assert_eq!(icon.image.dimensions(), (16, 16));
    assert_eq!(icon.image.get_pixel(3, 3).0, [255, 128, 0, 255]);
}

#[test]
fn oversized_or_unreadable_favicons_are_refused() {
    use super::favicon::{FaviconError, MAX_SIDE, sanitize};
    use image::ImageFormat;
    for (width, height) in [(MAX_SIDE + 1, 1), (1, MAX_SIDE + 1)] {
        assert_eq!(
            sanitize(&encoded(ImageFormat::Png, width, height, [0, 0, 0, 255])),
            Err(FaviconError::TooLarge)
        );
    }
    assert_eq!(
        sanitize(&encoded(ImageFormat::Gif, MAX_SIDE + 1, 8, [0, 0, 0, 255])),
        Err(FaviconError::TooLarge)
    );
    // A header too large to decode, refused before its pixels are read.
    assert_eq!(sanitize(&png(4096, 4096)), Err(FaviconError::TooLarge));
    // A sound header, but no pixels to decode.
    assert_eq!(sanitize(&png(16, 16)), Err(FaviconError::NotAnImage));
    assert_eq!(
        sanitize(b"\x89PNG\r\n\x1a\n"),
        Err(FaviconError::NotAnImage)
    );
    assert_eq!(
        sanitize(b"<svg width='99999'/>"),
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
    let served = encoded(image::ImageFormat::Png, 16, 16, [255, 0, 255, 255]);
    let huge_declared = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        MAX_BYTES + 1
    )
    .into_bytes();
    // No length: the body itself runs past the limit.
    let mut huge_streamed =
        b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nConnection: close\r\n\r\n".to_vec();
    huge_streamed.extend_from_slice(&served);
    huge_streamed.resize(huge_streamed.len() + MAX_BYTES, 0);
    let port = serve_bytes(vec![
        // A server that names the wrong type still serves an image.
        ("/icon.png", image_response("text/plain", &served)),
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
    assert_eq!(
        (icon.format, icon.image.dimensions()),
        (Format::Png, (16, 16))
    );
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
