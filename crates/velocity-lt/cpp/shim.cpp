// The C++ half of velocity-lt. Kept deliberately small: it translates plain
// structs to libtorrent calls and copies results back. Policy lives in Rust.

#include "velocity-lt/cpp/shim.h"
#include "velocity-lt/src/lib.rs.h"

#include <chrono>
#include <stdexcept>
#include <string>
#include <vector>

#include <libtorrent/add_torrent_params.hpp>
#include <libtorrent/alert_types.hpp>
#include <libtorrent/announce_entry.hpp>
#include <libtorrent/error_code.hpp>
#include <libtorrent/fingerprint.hpp>
#include <libtorrent/magnet_uri.hpp>
#include <libtorrent/peer_info.hpp>
#include <libtorrent/read_resume_data.hpp>
#include <libtorrent/session_params.hpp>
#include <libtorrent/settings_pack.hpp>
#include <libtorrent/torrent_info.hpp>
#include <libtorrent/torrent_status.hpp>
#include <libtorrent/address.hpp>
#include <libtorrent/socket.hpp>
#include <libtorrent/version.hpp>
#include <libtorrent/write_resume_data.hpp>

namespace velocity_lt {

namespace {

lt::settings_pack make_pack(SessionConfig const& cfg) {
    lt::settings_pack p;
    p.set_str(lt::settings_pack::listen_interfaces, std::string(cfg.listen_interfaces));
    p.set_bool(lt::settings_pack::enable_dht, cfg.enable_dht);
    p.set_bool(lt::settings_pack::enable_lsd, cfg.enable_lsd);
    p.set_bool(lt::settings_pack::enable_upnp, cfg.enable_upnp);
    p.set_bool(lt::settings_pack::enable_natpmp, cfg.enable_natpmp);
    if (!cfg.dht_bootstrap_nodes.empty())
        p.set_str(lt::settings_pack::dht_bootstrap_nodes, std::string(cfg.dht_bootstrap_nodes));
    if (!cfg.user_agent.empty())
        p.set_str(lt::settings_pack::user_agent, std::string(cfg.user_agent));
    if (!cfg.peer_fingerprint.empty())
        p.set_str(lt::settings_pack::peer_fingerprint, std::string(cfg.peer_fingerprint));
    p.set_int(lt::settings_pack::upload_rate_limit, cfg.upload_rate_limit);
    p.set_int(lt::settings_pack::download_rate_limit, cfg.download_rate_limit);
    if (cfg.connections_limit > 0)
        p.set_int(lt::settings_pack::connections_limit, cfg.connections_limit);
    // Passed as the flag type: MSVC will not static_cast it to int.
    p.set_int(lt::settings_pack::alert_mask,
        lt::alert_category::status | lt::alert_category::error
            | lt::alert_category::port_mapping | lt::alert_category::tracker
            | lt::alert_category::storage);
    // Torrents are scheduled by the application, not by libtorrent's queue.
    p.set_int(lt::settings_pack::active_downloads, -1);
    p.set_int(lt::settings_pack::active_seeds, -1);
    p.set_int(lt::settings_pack::active_limit, -1);
    return p;
}

std::string hex(lt::span<char const> s) {
    static char const digits[] = "0123456789abcdef";
    std::string out;
    out.reserve(std::size_t(s.size()) * 2);
    for (char c : s) {
        auto b = static_cast<unsigned char>(c);
        out.push_back(digits[b >> 4]);
        out.push_back(digits[b & 15]);
    }
    return out;
}

rust::String rstr(std::string const& s) {
    // libtorrent paths and messages are UTF-8 on every platform it supports, but
    // tracker messages come from the network; lossy keeps cxx from throwing.
    return rust::String::lossy(s);
}

} // namespace

Session::Session(SessionConfig const& cfg) {
    ses_ = std::make_unique<lt::session>(lt::session_params(make_pack(cfg)));
}

Session::~Session() {
    // lt::session's destructor waits for trackers to be told we stopped; abort
    // keeps shutdown quick when the app exits.
    if (ses_) {
        auto proxy = ses_->abort();
        ses_.reset();
    }
}

void Session::apply_config(SessionConfig const& cfg) const {
    ses_->apply_settings(make_pack(cfg));
}

lt::torrent_handle Session::handle(std::uint64_t id) const {
    std::lock_guard<std::mutex> g(mu_);
    auto it = by_id_.find(id);
    if (it == by_id_.end() || !it->second.is_valid())
        throw std::runtime_error("unknown torrent");
    return it->second;
}

std::uint64_t Session::id_of(lt::torrent_handle const& h) const {
    std::lock_guard<std::mutex> g(mu_);
    for (auto const& kv : by_id_)
        if (kv.second == h) return kv.first;
    return 0;
}

std::uint64_t Session::add(AddParams const& p) const {
    lt::add_torrent_params atp;
    lt::error_code ec;
    if (!p.resume_data.empty()) {
        lt::span<char const> buf(reinterpret_cast<char const*>(p.resume_data.data()),
            static_cast<std::ptrdiff_t>(p.resume_data.size()));
        atp = lt::read_resume_data(buf, ec);
        if (ec) throw std::runtime_error("resume data: " + ec.message());
    }
    if (!p.torrent.empty()) {
        lt::span<char const> buf(reinterpret_cast<char const*>(p.torrent.data()),
            static_cast<std::ptrdiff_t>(p.torrent.size()));
        auto ti = std::make_shared<lt::torrent_info>(buf, ec, lt::from_span);
        if (ec) throw std::runtime_error("torrent: " + ec.message());
        atp.ti = std::move(ti);
    } else if (!p.magnet.empty() && !atp.ti) {
        lt::add_torrent_params m = lt::parse_magnet_uri(std::string(p.magnet), ec);
        if (ec) throw std::runtime_error("magnet: " + ec.message());
        atp.info_hashes = m.info_hashes;
        atp.name = m.name;
        for (auto const& t : m.trackers) atp.trackers.push_back(t);
        atp.tracker_tiers = m.tracker_tiers;
    } else if (!atp.ti && atp.info_hashes.v1.is_all_zeros() && atp.info_hashes.v2.is_all_zeros()) {
        throw std::runtime_error("nothing to add");
    }
    atp.save_path = std::string(p.save_path);
    if (!p.file_priorities.empty()) {
        atp.file_priorities.clear();
        for (auto v : p.file_priorities) atp.file_priorities.push_back(lt::download_priority_t(v));
    }
    atp.flags &= ~lt::torrent_flags::auto_managed;
    atp.flags |= lt::torrent_flags::duplicate_is_error;
    if (p.paused) atp.flags |= lt::torrent_flags::paused;
    else atp.flags &= ~lt::torrent_flags::paused;
    if (p.disable_dht) atp.flags |= lt::torrent_flags::disable_dht;
    if (p.disable_lsd) atp.flags |= lt::torrent_flags::disable_lsd;
    if (p.disable_pex) atp.flags |= lt::torrent_flags::disable_pex;
    if (p.dont_download) atp.flags |= lt::torrent_flags::default_dont_download;
    atp.upload_limit = p.upload_limit > 0 ? p.upload_limit : -1;
    atp.download_limit = p.download_limit > 0 ? p.download_limit : -1;
    if (p.max_connections > 0) atp.max_connections = p.max_connections;
    int tier = atp.tracker_tiers.empty() ? 0 : atp.tracker_tiers.back() + 1;
    atp.tracker_tiers.resize(atp.trackers.size(), 0);
    for (auto const& t : p.trackers) {
        std::string url(t);
        bool known = false;
        for (auto const& e : atp.trackers) known = known || e == url;
        if (known) continue;
        atp.trackers.push_back(url);
        atp.tracker_tiers.push_back(tier++);
    }

    lt::torrent_handle h = ses_->add_torrent(std::move(atp), ec);
    if (ec == lt::errors::duplicate_torrent) throw std::runtime_error("duplicate_torrent");
    if (ec) throw std::runtime_error(ec.message());
    std::lock_guard<std::mutex> g(mu_);
    std::uint64_t id = next_id_++;
    by_id_.emplace(id, h);
    return id;
}

bool Session::remove(std::uint64_t id) const {
    lt::torrent_handle h;
    {
        std::lock_guard<std::mutex> g(mu_);
        auto it = by_id_.find(id);
        if (it == by_id_.end()) return false;
        h = it->second;
        by_id_.erase(it);
    }
    // No delete_files flag: data is never removed by the engine.
    if (h.is_valid()) ses_->remove_torrent(h);
    return true;
}

TorrentStatus Session::status(std::uint64_t id) const {
    lt::torrent_status s = handle(id).status(lt::torrent_handle::query_name
        | lt::torrent_handle::query_save_path | lt::torrent_handle::query_torrent_file);
    TorrentStatus o{};
    o.state = static_cast<std::uint8_t>(s.state);
    o.paused = bool(s.flags & lt::torrent_flags::paused);
    o.has_metadata = s.has_metadata;
    o.is_finished = s.is_finished;
    o.is_seeding = s.is_seeding;
    o.total_wanted = s.total_wanted;
    o.total_wanted_done = s.total_wanted_done;
    o.total_done = s.total_done;
    o.all_time_upload = s.all_time_upload;
    o.all_time_download = s.all_time_download;
    o.download_rate = s.download_payload_rate;
    o.upload_rate = s.upload_payload_rate;
    o.num_peers = s.num_peers;
    o.num_seeds = s.num_seeds;
    o.num_complete = s.num_complete;
    o.num_incomplete = s.num_incomplete;
    if (s.errc) o.error = rstr(s.errc.message());
    o.name = rstr(s.name);
    o.save_path = rstr(s.save_path);
    if (auto ti = s.torrent_file.lock()) {
        o.num_pieces = ti->num_pieces();
        o.piece_length = ti->piece_length();
    }
    if (s.info_hashes.has_v1()) o.info_hash_v1 = hex(s.info_hashes.v1);
    if (s.info_hashes.has_v2()) o.info_hash_v2 = hex(s.info_hashes.v2);
    o.seeding_secs = std::chrono::duration_cast<std::chrono::seconds>(s.seeding_duration).count();
    o.need_save_resume = s.need_save_resume;
    return o;
}

rust::Vec<FileEntry> Session::files(std::uint64_t id) const {
    lt::torrent_handle h = handle(id);
    rust::Vec<FileEntry> out;
    auto ti = h.torrent_file();
    if (!ti) return out;
    std::string save = h.status(lt::torrent_handle::query_save_path).save_path;
    lt::file_storage const& fs = ti->files();
    std::vector<std::int64_t> progress;
    h.file_progress(progress);
    auto prio = h.get_file_priorities();
    for (lt::file_index_t i : fs.file_range()) {
        FileEntry e{};
        e.path = rstr(fs.file_path(i, save));
        e.size = fs.file_size(i);
        int n = static_cast<int>(i);
        e.done = n < int(progress.size()) ? progress[std::size_t(n)] : 0;
        e.priority = n < int(prio.size()) ? static_cast<std::uint8_t>(prio[std::size_t(n)]) : 4;
        e.pad = fs.pad_file_at(i);
        out.push_back(std::move(e));
    }
    return out;
}

rust::Vec<PeerEntry> Session::peers(std::uint64_t id) const {
    std::vector<lt::peer_info> v;
    handle(id).get_peer_info(v);
    rust::Vec<PeerEntry> out;
    for (auto const& p : v) {
        PeerEntry e{};
        e.ip = rstr(p.ip.address().to_string());
        e.port = p.ip.port();
        e.download_rate = p.payload_down_speed;
        e.upload_rate = p.payload_up_speed;
        e.seed = bool(p.flags & lt::peer_info::seed);
        e.am_choking = bool(p.flags & lt::peer_info::choked);
        e.peer_choking = bool(p.flags & lt::peer_info::remote_choked);
        e.progress = p.progress;
        e.client = rstr(p.client);
        out.push_back(std::move(e));
    }
    return out;
}

rust::Vec<TrackerEntry> Session::trackers(std::uint64_t id) const {
    rust::Vec<TrackerEntry> out;
    auto now = lt::clock_type::now();
    for (auto const& ae : handle(id).trackers()) {
        TrackerEntry e{};
        e.url = rstr(ae.url);
        e.tier = ae.tier;
        e.next_announce_secs = -1;
        e.scrape_complete = -1;
        e.scrape_incomplete = -1;
        bool any_ok = false;
        // One entry per listen socket (IPv4, IPv6) and protocol version. A tracker
        // works if any of them works; the tracker's own message (a failure reason
        // or warning) is more useful than a local socket error such as an IPv6
        // socket that cannot reach an IPv4 tracker.
        std::string tracker_msg, local_err;
        for (auto const& ep : ae.endpoints) {
            for (auto const& ih : ep.info_hashes) {
                if (ih.updating) e.updating = true;
                if (ih.fails > e.fails) e.fails = ih.fails;
                if (ih.fails == 0 && ih.start_sent) any_ok = true;
                if (!ih.message.empty()) tracker_msg = ih.message;
                else if (ih.last_error) local_err = ih.last_error.message();
                if (ih.scrape_complete > e.scrape_complete) e.scrape_complete = ih.scrape_complete;
                if (ih.scrape_incomplete > e.scrape_incomplete) e.scrape_incomplete = ih.scrape_incomplete;
                if (ih.next_announce > now) {
                    auto secs = std::chrono::duration_cast<std::chrono::seconds>(ih.next_announce - now).count();
                    if (e.next_announce_secs < 0 || secs < e.next_announce_secs) e.next_announce_secs = secs;
                }
            }
        }
        e.working = any_ok;
        if (!tracker_msg.empty()) e.message = rstr(tracker_msg);
        else if (!any_ok && !local_err.empty()) e.message = rstr(local_err);
        out.push_back(std::move(e));
    }
    return out;
}

void Session::pause(std::uint64_t id) const { handle(id).pause(lt::torrent_handle::graceful_pause); }
void Session::resume(std::uint64_t id) const { handle(id).resume(); }

void Session::set_file_priorities(std::uint64_t id, rust::Slice<std::uint8_t const> priorities) const {
    std::vector<lt::download_priority_t> v;
    for (auto p : priorities) v.push_back(lt::download_priority_t(p));
    handle(id).prioritize_files(v);
}

void Session::set_limits(std::uint64_t id, std::int32_t upload, std::int32_t download) const {
    auto h = handle(id);
    h.set_upload_limit(upload > 0 ? upload : -1);
    h.set_download_limit(download > 0 ? download : -1);
}

void Session::force_reannounce(std::uint64_t id) const { handle(id).force_reannounce(0, -1, lt::torrent_handle::ignore_min_interval); }
void Session::force_recheck(std::uint64_t id) const { handle(id).force_recheck(); }

void Session::flush(std::uint64_t id) const { handle(id).flush_cache(); }

void Session::request_resume_data(std::uint64_t id) const {
    handle(id).save_resume_data(lt::torrent_handle::save_info_dict);
}

rust::Vec<std::uint8_t> Session::metadata(std::uint64_t id) const {
    auto ti = handle(id).torrent_file();
    if (!ti) throw std::runtime_error("metadata is not available yet");
    auto info = ti->info_section();
    rust::Vec<std::uint8_t> out;
    out.reserve(std::size_t(info.size()) + 8);
    for (char c : std::string("d4:info")) out.push_back(static_cast<std::uint8_t>(c));
    for (char c : info) out.push_back(static_cast<std::uint8_t>(c));
    out.push_back(static_cast<std::uint8_t>('e'));
    return out;
}

rust::Vec<AlertEntry> Session::pop_alerts() const {
    std::vector<lt::alert*> alerts;
    ses_->pop_alerts(&alerts);
    rust::Vec<AlertEntry> out;
    for (lt::alert* a : alerts) {
        AlertEntry e{};
        if (auto* x = lt::alert_cast<lt::listen_succeeded_alert>(a)) {
            e.kind = "listen_succeeded";
            e.port = x->port;
            e.transport = x->socket_type == lt::socket_type_t::tcp ? "tcp" : "udp";
        } else if (auto* x = lt::alert_cast<lt::listen_failed_alert>(a)) {
            e.kind = "listen_failed";
            e.port = x->port;
            e.message = rstr(x->error.message());
        } else if (auto* x = lt::alert_cast<lt::portmap_alert>(a)) {
            e.kind = "portmap";
            e.port = x->external_port;
            e.transport = x->map_transport == lt::portmap_transport::upnp ? "upnp" : "natpmp";
            e.protocol = x->map_protocol == lt::portmap_protocol::udp ? "udp" : "tcp";
        } else if (auto* x = lt::alert_cast<lt::portmap_error_alert>(a)) {
            e.kind = "portmap_error";
            e.transport = x->map_transport == lt::portmap_transport::upnp ? "upnp" : "natpmp";
            e.message = rstr(x->error.message());
        } else if (auto* x = lt::alert_cast<lt::metadata_received_alert>(a)) {
            e.kind = "metadata_received";
            e.id = id_of(x->handle);
        } else if (auto* x = lt::alert_cast<lt::torrent_finished_alert>(a)) {
            e.kind = "torrent_finished";
            e.id = id_of(x->handle);
        } else if (auto* x = lt::alert_cast<lt::cache_flushed_alert>(a)) {
            e.kind = "cache_flushed";
            e.id = id_of(x->handle);
        } else if (auto* x = lt::alert_cast<lt::file_error_alert>(a)) {
            e.kind = "file_error";
            e.id = id_of(x->handle);
            e.message = rstr(x->error.message());
        } else if (auto* x = lt::alert_cast<lt::torrent_error_alert>(a)) {
            e.kind = "torrent_error";
            e.id = id_of(x->handle);
            e.message = rstr(x->error.message());
        } else if (auto* x = lt::alert_cast<lt::tracker_error_alert>(a)) {
            e.kind = "tracker_error";
            e.id = id_of(x->handle);
            e.url = rstr(x->tracker_url());
            std::string reason = x->failure_reason();
            e.message = rstr(reason.empty() ? x->error.message() : reason);
        } else if (auto* x = lt::alert_cast<lt::tracker_warning_alert>(a)) {
            e.kind = "tracker_warning";
            e.id = id_of(x->handle);
            e.url = rstr(x->tracker_url());
            e.message = rstr(x->warning_message());
        } else if (auto* x = lt::alert_cast<lt::tracker_reply_alert>(a)) {
            e.kind = "tracker_reply";
            e.id = id_of(x->handle);
            e.url = rstr(x->tracker_url());
            e.port = x->num_peers;
        } else if (auto* x = lt::alert_cast<lt::save_resume_data_alert>(a)) {
            e.kind = "save_resume_data";
            e.id = id_of(x->handle);
            auto buf = lt::write_resume_data_buf(x->params);
            for (char c : buf) e.data.push_back(static_cast<std::uint8_t>(c));
        } else if (auto* x = lt::alert_cast<lt::save_resume_data_failed_alert>(a)) {
            e.kind = "save_resume_data_failed";
            e.id = id_of(x->handle);
            e.message = rstr(x->error.message());
        } else if (lt::alert_cast<lt::torrent_removed_alert>(a)) {
            // The id was dropped when remove() was called; report the hash-less event.
            e.kind = "torrent_removed";
        } else {
            continue;
        }
        out.push_back(std::move(e));
    }
    return out;
}

void Session::connect_peer(std::uint64_t id, rust::Str ip, std::uint16_t port) const {
    lt::error_code ec;
    auto addr = lt::make_address(std::string(ip), ec);
    if (ec) throw std::runtime_error("address: " + ec.message());
    handle(id).connect_peer(lt::tcp::endpoint(addr, port));
}

std::uint16_t Session::listen_port() const { return ses_->listen_port(); }

std::unique_ptr<Session> new_session(SessionConfig const& cfg) {
    return std::make_unique<Session>(cfg);
}

rust::String version() { return rust::String(lt::version()); }

} // namespace velocity_lt
