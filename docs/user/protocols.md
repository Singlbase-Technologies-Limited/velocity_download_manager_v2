# Protocols and limits

What V2 downloads, and what it can do for each kind of link. The tables below are checked against what V2's download engine reports about itself, so they change only when the engine does (`crates/velocity-transfer/tests/user_guide.rs`).

## Direct downloads

| Protocol | Links | Several connections | Resumes | Resume checked against the file | HTTP/2 | Encrypted |
|---|---|---|---|---|---|---|
| HTTP and HTTPS | `http://`, `https://` | Yes | Yes | Yes | Yes | Yes |
| FTP and FTPS | `ftp://`, `ftps://` | No | Yes | Yes | No | Yes |
| SFTP | `sftp://` | No | Yes | Yes | No | Yes |

What the columns mean:

- **Several connections:** V2 splits the file and downloads its parts over several connections at once. It starts with all the connections a download may use (8 by default) and keeps only those that make it faster. FTP and SFTP use one connection per download for now.
- **Resumes:** a paused download, or one interrupted by a restart, a crash or a power cut, goes on from the last progress V2 saved, not from the start.
- **Resume checked against the file:** before resuming, V2 checks the file on the server is still the same one (its ETag or modification time). If it changed, V2 starts over rather than mix two versions.
- **HTTP/2:** with a server that speaks HTTP/2, several parts share one connection. V2 tries separate connections too, and keeps whichever is faster.
- **Encrypted:**
  - **HTTPS:** always, and the server's certificate is always checked. There is no way to turn that check off.
  - **FTP:** an `ftps://` link is encrypted. A plain `ftp://` link is encrypted only when you tick "Require encryption (FTPS)" when adding it. Without that, the Add dialog warns that the login and the file can be read on the network.
  - **SFTP:** always. The first time you download from a server, V2 asks you to confirm the server's identity ([Downloads](downloads.md#sftp-servers)).

**Limits that apply to every direct download:**

- **Redirects** are followed only to `http://` and `https://` links, at most 10. A redirect from `https://` to `http://` stops and asks first ([Troubleshooting](troubleshooting.md#a-download-stopped-and-asks-something)).
- **No proxy or `.netrc` from the system:** V2 uses only the proxy set in its own Settings ([Settings](settings.md#proxy)).
- **Checksums:** when you give one (MD5, SHA-1, SHA-224, SHA-256, SHA-384 or SHA-512), the finished file is checked against it, and the download is marked verified.

## Metalink

| Feature | Supported |
|---|---|
| Metalink files | Yes |

A Metalink file lists one or more files, with their mirrors and checksums. When the Metalink switch is on (Settings, Advanced; off by default, as in V1), a link to a `.metalink` or `.meta4` file adds the files it describes, and V2 downloads each from its mirrors and checks it against the checksums the Metalink file gives.

**Not yet:** adding a Metalink file that is already on your computer. Only a link to one works (#222).

## BitTorrent

Torrents and magnet links are downloaded by a separate program, the torrent helper, which uses libtorrent. It supports BitTorrent v1, v2 and hybrid torrents, magnet links, DHT, peer exchange, choosing files, and seeding. The [Torrents](torrents.md) guide covers them. Unlike the table above, this part isn't checked against the engine automatically.

## Not supported

- **Other links,** such as `file://`, `data:` or `scp://`, are refused in the Add dialog, with the reason.
- **Several connections for FTP and SFTP:** each downloads over one connection for now.
- **An HTTPS proxy for SFTP and torrents:** choose an HTTP or SOCKS5 proxy for those ([Settings](settings.md#proxy)).
