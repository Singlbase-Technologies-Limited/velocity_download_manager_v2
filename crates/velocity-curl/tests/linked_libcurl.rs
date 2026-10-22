//! The libcurl that is actually linked: the Velocity-built one from `pnpm native:build`
//! (docs/BUILDING_NATIVE.md), never curl-sys's bundled curl 8.22.0 or a system copy.

use velocity_curl::{Libcurl, EXPECTED_TLS, LIBCURL_FLOOR};

#[test]
fn starts_on_the_velocity_built_libcurl() {
    let curl = Libcurl::start().unwrap_or_else(|e| panic!("libcurl should start: {e}"));
    let linked = curl.linked();
    let (major, minor, patch) = LIBCURL_FLOOR;
    let version: Vec<u32> = linked
        .version
        .split('.')
        .map(|p| p.parse().expect("numeric version"))
        .collect();
    assert!(
        version >= vec![major, minor, patch],
        "linked libcurl {} is below the floor",
        linked.version
    );
    assert_eq!(linked.tls.split('/').next(), Some(EXPECTED_TLS));
    let mut protocols = linked.protocols.clone();
    protocols.sort();
    assert_eq!(protocols, ["ftp", "ftps", "http", "https"]);
    for feature in ["HTTP2", "SSL", "libz"] {
        assert!(
            linked.features.iter().any(|f| f == feature),
            "missing {feature} in {:?}",
            linked.features
        );
    }
}

#[test]
fn a_second_start_reports_the_same_libcurl() {
    let first = Libcurl::start().expect("first start").linked().clone();
    let second = Libcurl::start().expect("second start").linked().clone();
    assert_eq!(first, second);
}
