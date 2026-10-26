#!/usr/bin/env python3
"""Generate the BitTorrent test corpus in tests/fixtures/torrents/.

Uses libtorrent 2.0's own torrent creator (python3-libtorrent), so v2 Merkle trees,
piece layers and hybrid padding come from a proven implementation, not from us.
File contents are deterministic (the same `content(seed, len)` as the tests in
crates/velocity-lt/tests and crates/velocity-torrent/tests), so tests regenerate the data and compare
SHA-256 digests instead of storing it.

    python3 scripts/make-torrent-corpus.py   # needs `import libtorrent` 2.0.x

The output is committed; rerun only when the corpus changes. A generated
manifest.json records what libtorrent itself says about every torrent (file list
with padding flags, v1/v2 hashes, whether it loads), which the Rust parser tests
use as the reference.
"""

import hashlib
import json
import os
import shutil
import sys
import tempfile

import libtorrent as lt

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "tests", "fixtures", "torrents")
PIECE = 16 * 1024
MASK = (1 << 64) - 1


def content(seed, length):
    out = bytearray(length)
    for i in range(length):
        x = ((i + seed * 7919) * 2654435761) & MASK
        out[i] = ((x >> 9) ^ (x >> 17) ^ seed) & 0xFF
    return bytes(out)


# name -> (single file?, [(relative path inside the torrent, length, seed)])
LAYOUTS = {
    "single": (True, [("payload.bin", 200_000, 1)]),
    "multi": (
        False,
        [
            ("a.bin", 150_000, 2),
            ("empty.txt", 0, 3),
            ("exact.bin", 2 * PIECE, 4),
            ("sub/b.bin", 70_000, 5),
            ("sub/deeper/c.bin", 5_000, 6),
        ],
    ),
}

KINDS = {"v1": lt.create_torrent.v1_only, "v2": lt.create_torrent.v2_only, "hybrid": 0}


def build(tmp, kind, layout):
    single, files = LAYOUTS[layout]
    name = f"{kind}-{layout}"
    root = os.path.join(tmp, name)
    for rel, length, seed in files:
        p = root if single else os.path.join(root, *rel.split("/"))
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(content(seed, length))
    fs = lt.file_storage()
    lt.add_files(fs, root)
    t = lt.create_torrent(fs, PIECE, flags=KINDS[kind])
    t.set_creator("velocity-test-corpus")
    lt.set_piece_hashes(t, tmp)
    d = t.generate()
    d.pop(b"creation date", None)  # outside the info dict; dropped for reproducible bytes
    return name, lt.bencode(d), single, files


def describe(data):
    """What libtorrent makes of these bytes."""
    try:
        ti = lt.torrent_info(lt.bdecode(data) if isinstance(data, bytes) else data)
    except Exception as e:  # noqa: BLE001 - recorded as evidence
        return {"loads": False, "error": str(e)}
    ih = ti.info_hashes()
    fs = ti.files()
    return {
        "loads": True,
        "v1": str(ih.v1) if ih.has_v1() else None,
        "v2": str(ih.v2) if ih.has_v2() else None,
        "pieces": ti.num_pieces(),
        "files": [
            {
                "path": fs.file_path(i).replace("\\", "/"),
                "length": fs.file_size(i),
                "pad": bool(fs.file_flags(i) & lt.file_storage.flag_pad_file),
            }
            for i in range(fs.num_files())
        ],
    }


def write(name, data):
    with open(os.path.join(OUT, name), "wb") as f:
        f.write(data)


def main():
    if not lt.__version__.startswith("2."):
        sys.exit(f"libtorrent 2.x is required, found {lt.__version__}")
    if os.path.isdir(OUT):
        shutil.rmtree(OUT)
    os.makedirs(OUT)
    manifest = {"generator": f"libtorrent {lt.__version__}", "pieceLength": PIECE, "torrents": {}, "magnets": {}, "invalid": {}}
    good = {}
    with tempfile.TemporaryDirectory() as tmp:
        for kind in KINDS:
            for layout in LAYOUTS:
                name, data, single, files = build(tmp, kind, layout)
                good[name] = data
                write(name + ".torrent", data)
                entry = describe(data)
                assert entry["loads"], (name, entry)
                entry["kind"] = kind
                entry["singleFile"] = single
                entry["content"] = [
                    {"path": rel, "length": length, "seed": seed, "sha256": hashlib.sha256(content(seed, length)).hexdigest()}
                    for rel, length, seed in files
                ]
                manifest["torrents"][name] = entry
                ti = lt.torrent_info(lt.bdecode(data))
                manifest["magnets"][name] = lt.make_magnet_uri(ti)

    # --- invalid and misleading inputs -----------------------------------------
    def variant(base, why, mutate):
        d = lt.bdecode(good[base])
        mutate(d)
        data = lt.bencode(d)
        fname = why + ".torrent"
        write(fname, data)
        manifest["invalid"][fname] = {"base": base, "libtorrent": describe(data)}

    def flip_layer(d):
        layers = d[b"piece layers"]
        k = sorted(layers.keys())[0]
        v = bytearray(layers[k])
        v[0] ^= 0xFF
        layers[k] = bytes(v)

    def drop_layer(d):
        layers = d[b"piece layers"]
        del layers[sorted(layers.keys())[0]]

    def short_layer(d):
        layers = d[b"piece layers"]
        k = sorted(layers.keys())[0]
        layers[k] = layers[k][:-32]

    def meta_version_3(d):
        d[b"info"][b"meta version"] = 3

    def hybrid_length_mismatch(d):
        # v1 list says a.bin is one byte shorter than the v2 file tree does.
        for f in d[b"info"][b"files"]:
            if f[b"path"] == [b"a.bin"]:
                f[b"length"] -= 1

    def hybrid_name_mismatch(d):
        for f in d[b"info"][b"files"]:
            if f[b"path"] == [b"a.bin"]:
                f[b"path"] = [b"renamed.bin"]

    def traversal(d):
        tree = d[b"info"][b"file tree"]
        tree[b".."] = tree.pop(b"a.bin")

    def bad_root_size(d):
        tree = d[b"info"][b"file tree"]
        tree[b"a.bin"][b""][b"pieces root"] = b"\x01" * 31

    variant("v2-multi", "v2-piece-layer-corrupt", flip_layer)
    variant("v2-multi", "v2-piece-layer-missing", drop_layer)
    variant("v2-multi", "v2-piece-layer-short", short_layer)
    variant("v2-multi", "v2-meta-version-3", meta_version_3)
    variant("v2-multi", "v2-path-traversal", traversal)
    variant("v2-multi", "v2-pieces-root-size", bad_root_size)
    variant("hybrid-multi", "hybrid-length-mismatch", hybrid_length_mismatch)
    variant("hybrid-multi", "hybrid-name-mismatch", hybrid_name_mismatch)

    v1h = manifest["torrents"]["v1-multi"]["v1"]
    v2h = manifest["torrents"]["v2-multi"]["v2"]
    manifest["badMagnets"] = {
        # A btmh multihash must be sha2-256 (0x12) of 32 bytes (0x20).
        "wrong-multihash-code": f"magnet:?xt=urn:btmh:1120{v2h}",
        "short-v2-hash": f"magnet:?xt=urn:btmh:1220{v2h[:-2]}",
        "non-hex-v2-hash": f"magnet:?xt=urn:btmh:1220{'zz' * 32}",
        # Two v2 hashes that disagree.
        "two-v2-hashes": f"magnet:?xt=urn:btmh:1220{v2h}&xt=urn:btmh:1220{'0' * 64}",
        # A v1 hash of one torrent paired with the v2 hash of another is accepted
        # as a magnet (they can't be checked until metadata arrives) but must be
        # refused once the metadata shows they don't belong together.
        "mismatched-pair": f"magnet:?xt=urn:btih:{v1h}&xt=urn:btmh:1220{v2h}",
    }
    with open(os.path.join(OUT, "manifest.json"), "w") as f:
        json.dump(manifest, f, indent=2, sort_keys=True)
        f.write("\n")
    print(f"wrote {len(os.listdir(OUT))} files to {os.path.normpath(OUT)}")


if __name__ == "__main__":
    main()
