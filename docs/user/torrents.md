# Torrents

Velocity downloads torrents and magnet links with a separate program, the torrent helper, built on libtorrent. If it ever stops, Velocity starts it again and the torrents carry on. BitTorrent can be turned off in Settings, BitTorrent; torrents and magnet links are then refused.

## Adding a torrent

Use the Add dialog ([Downloads](downloads.md#adding-downloads)): paste a magnet link or a link to a `.torrent` file, choose `.torrent` files with **Choose .torrent files…**, or drop them on the window.

- **One `.torrent` file** shows its name, its version (v1, v2 or hybrid), its hash and its files as a tree, all chosen at first. Untick what you don't want; a folder's box chooses everything in it.
- **Several `.torrent` files** are each added whole.
- **A magnet link** has no file list until the torrent's details arrive from other peers. With **Choose files before downloading** ticked, the torrent waits in **Choose files** once they do; otherwise every file is downloaded.
- **Seeding** and an **upload limit** can be set for each torrent here ([Seeding](#seeding)). The defaults come from Settings, BitTorrent, New torrents.
- **Already added:** Velocity warns when a torrent is already in the list or in the history, and offers **Add anyway**. A torrent still in the list can't be added twice.
- **Names:** a torrent's file names come from the torrent and can't be changed, so a name already taken needs another folder.

## Choosing files later

A torrent in **Choose files** waits for you: double-click it, or use its menu or its details. In the details panel you can also change the files of a torrent at any time. Files you drop are kept on disk unless you ask Velocity to delete them.

## Seeding

Seeding is off by default: a torrent stops once it is complete. Each torrent can instead:

| Choice | Seeds until |
|---|---|
| Never | It completes (the default) |
| For a time | 1 minute to 365 days |
| To a ratio | It has uploaded 0.1 to 100 times what it downloaded |
| Time or ratio | Whichever comes first |
| Without limit | You stop it |

- **Seeding torrents** are listed under **Seeding**. **Stop seeding** stops one.
- **The seeding limit** in Settings, BitTorrent caps how many seed at once (no limit by default). The others wait in a queue, longest-waiting first; **Leave seeding queue** takes one out.
- **The time and ratio** count over the torrent's whole life, so a restart doesn't reset them.

## In the details panel

For a torrent, the details panel shows the peers and seeds connected, what it uploaded, its upload speed and time seeding, its hashes, version and seeding policy, each tracker's state with **Re-announce**, the connected peers, and its upload limit, which you can change.

**Tracker passkeys are hidden:** a private tracker's address can carry a personal key. Velocity shows only a tracker's name and port, and never writes the rest to its log.

## Limits and settings

Settings, BitTorrent holds the torrent settings: DHT, peer exchange and local peer discovery, peers per torrent, the seeding limit, upload and download limits for all torrents together, extra trackers, and the listening port (6890 by default) with port mapping on your router. They are listed in [Settings](settings.md#bittorrent).

## Not yet, or different from V1

- **Private torrents after a restart:** Velocity doesn't keep tracker addresses, which can carry passkeys, when it closes. A private torrent then finds no peers; add it again from its `.torrent` file or magnet link.
- **Connections** for a torrent are the torrent helper's own, so a torrent's details don't offer a connection count.
- **Through a proxy,** torrents don't look for peers on the local network or map a port, use the DHT only through a SOCKS5 proxy, and can't use an HTTPS proxy at all ([Settings](settings.md#proxy)).
