//! A separate process prevents telemetry or another test from selecting TLS first.

#[test]
fn clients_can_select_tls_without_enabling_telemetry() {
    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    buddy_core::http::initialize_tls();
    let selected = rustls::crypto::CryptoProvider::get_default().unwrap();
    let _ = rustls::ClientConfig::builder();
    buddy_core::http::initialize_tls();
    assert!(std::sync::Arc::ptr_eq(
        selected,
        rustls::crypto::CryptoProvider::get_default().unwrap()
    ));
}
