//! The user guide's protocol page (#79, `docs/user/protocols.md`) says what this engine reports:
//! a row for each protocol it downloads and no other, with the same answers, and the same for
//! Metalink. A change to the engine's capabilities fails here until the page says it too.

use velocity_core::domain::{DownloadKind, ProtocolCapabilities};
use velocity_transfer::engine::{protocols, METALINK};

const PAGE: &str = include_str!("../../../docs/user/protocols.md");

/// The rows of the first table after `heading`, as cells, without the header and its rule.
fn table_after(heading: &str) -> Vec<Vec<String>> {
    let start = PAGE
        .find(heading)
        .unwrap_or_else(|| panic!("protocols.md has no {heading:?}"));
    PAGE[start..]
        .lines()
        .skip_while(|l| !l.starts_with('|'))
        .take_while(|l| l.starts_with('|'))
        .skip(2)
        .map(|l| {
            l.trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_string())
                .collect()
        })
        .collect()
}

/// The page's name for a protocol's row.
fn row_name(kind: DownloadKind) -> &'static str {
    match kind {
        DownloadKind::Http => "HTTP and HTTPS",
        DownloadKind::Ftp => "FTP and FTPS",
        DownloadKind::Sftp => "SFTP",
        other => panic!("{other:?} isn't a direct download protocol"),
    }
}

fn yes_no(b: bool) -> &'static str {
    if b {
        "Yes"
    } else {
        "No"
    }
}

/// The cells the page must show for `p`, after its name and its links.
fn cells(p: &ProtocolCapabilities) -> [&'static str; 5] {
    [
        yes_no(p.segmented),
        yes_no(p.resume),
        yes_no(p.validator_resume),
        yes_no(p.http2),
        yes_no(p.encryption),
    ]
}

#[test]
fn the_protocol_page_lists_what_the_engine_downloads_and_nothing_else() {
    let rows = table_after("## Direct downloads");
    let listed: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();
    let engine: Vec<&str> = protocols().iter().map(|p| row_name(p.kind)).collect();
    assert_eq!(listed, engine);
}

#[test]
fn the_protocol_page_says_what_the_engine_does_for_each() {
    let rows = table_after("## Direct downloads");
    for p in protocols() {
        let row = rows
            .iter()
            .find(|r| r[0] == row_name(p.kind))
            .expect("listed");
        assert_eq!(row[2..], cells(&p), "{}", row_name(p.kind));
    }
}

#[test]
fn the_protocol_page_says_whether_metalink_files_are_read() {
    let rows = table_after("## Metalink");
    assert_eq!(
        rows,
        [vec![
            "Metalink files".to_string(),
            yes_no(METALINK).to_string()
        ]]
    );
}
