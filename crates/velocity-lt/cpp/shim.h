#pragma once

#include <cstdint>
#include <memory>
#include <mutex>
#include <unordered_map>

#include "rust/cxx.h"

#include <libtorrent/session.hpp>
#include <libtorrent/torrent_handle.hpp>

namespace velocity_lt {

struct SessionConfig;
struct AddParams;
struct TorrentStatus;
struct FileEntry;
struct PeerEntry;
struct TrackerEntry;
struct AlertEntry;

class Session {
public:
    explicit Session(SessionConfig const& cfg);
    ~Session();

    void apply_config(SessionConfig const& cfg) const;
    std::uint64_t add(AddParams const& p) const;
    bool remove(std::uint64_t id) const;
    TorrentStatus status(std::uint64_t id) const;
    rust::Vec<FileEntry> files(std::uint64_t id) const;
    rust::Vec<PeerEntry> peers(std::uint64_t id) const;
    rust::Vec<TrackerEntry> trackers(std::uint64_t id) const;
    void pause(std::uint64_t id) const;
    void resume(std::uint64_t id) const;
    void set_file_priorities(std::uint64_t id, rust::Slice<std::uint8_t const> priorities) const;
    void set_limits(std::uint64_t id, std::int32_t upload, std::int32_t download) const;
    void force_reannounce(std::uint64_t id) const;
    void force_recheck(std::uint64_t id) const;
    void flush(std::uint64_t id) const;
    void request_resume_data(std::uint64_t id) const;
    rust::Vec<std::uint8_t> metadata(std::uint64_t id) const;
    rust::Vec<AlertEntry> pop_alerts() const;
    void connect_peer(std::uint64_t id, rust::Str ip, std::uint16_t port) const;
    std::uint16_t listen_port() const;

private:
    lt::torrent_handle handle(std::uint64_t id) const;
    std::uint64_t id_of(lt::torrent_handle const& h) const;

    std::unique_ptr<lt::session> ses_;
    mutable std::mutex mu_;
    mutable std::uint64_t next_id_ = 1;
    mutable std::unordered_map<std::uint64_t, lt::torrent_handle> by_id_;
};

std::unique_ptr<Session> new_session(SessionConfig const& cfg);
rust::String version();

} // namespace velocity_lt
