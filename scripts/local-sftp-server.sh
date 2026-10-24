#!/usr/bin/env bash
# Local only, on a Linux developer machine such as the PO's WSL (#176): the password-login SFTP
# server that CI's Linux job provisions with scripts/ci-sftp-server.sh, as a lasting service, so
# the password-login tests run locally too while CI is paused (#173).
#
# Run it once, as the user who runs the tests:   sudo bash scripts/local-sftp-server.sh
# Remove everything it set up:                   sudo bash scripts/local-sftp-server.sh --remove
#
# - A test user, v2sftp, with no shell and a random password, shut in its own folder
#   (ChrootDirectory), which holds one file of random bytes.
# - Its own sshd, as the systemd service velocity-v2-sftp, on 127.0.0.1:2222: password logins for
#   v2sftp only, internal-sftp only. Its configuration lives in /etc/velocity-v2-sftp, so the
#   machine's own sshd is never touched.
# - The server's description, password included, in ~/.config/velocity-v2/sftp-password-server.env
#   of the user who ran sudo, readable by that user only. The local Linux check exports it as
#   VELOCITY_SFTP_PASSWORD_SERVER (.claude/skills/platform-verify/reference.md).
#
# Running it again keeps the user, the password and the file, and only checks that the server
# answers. The password never appears in a process argument or in the output.
set -euo pipefail

user=v2sftp
service=velocity-v2-sftp
conf_dir=/etc/velocity-v2-sftp
root_dir=/var/lib/velocity-v2-sftp
unit=/etc/systemd/system/$service.service
port=2222

die() {
  echo "local-sftp-server: $*" >&2
  exit 1
}

[ "$(uname -s)" = Linux ] || die "this sets up a Linux service; run it in WSL or on Linux"
[ "$(id -u)" = 0 ] || die "run it with sudo: sudo bash scripts/local-sftp-server.sh"
owner="${SUDO_USER:-}"
[ -n "$owner" ] && [ "$owner" != root ] ||
  die "run it with sudo from the account that runs the tests, not as root directly"
command -v systemctl >/dev/null && [ "$(ps -p 1 -o comm=)" = systemd ] ||
  die "systemd isn't running (WSL: set systemd=true under [boot] in /etc/wsl.conf)"
owner_home="$(getent passwd "$owner" | cut -d : -f 6)"
[ -d "$owner_home" ] || die "no home folder for $owner"
env_dir="$owner_home/.config/velocity-v2"
env_file="$env_dir/sftp-password-server.env"

# Files in the owner's home are read and written as the owner, never as root, so a link there
# can't point root at another file.
as_owner() {
  runuser -u "$owner" -- "$@"
}

if [ "${1:-}" = --remove ]; then
  systemctl disable --now "$service" 2>/dev/null || true
  rm -f "$unit"
  systemctl daemon-reload
  rm -rf "$conf_dir" "$root_dir"
  if id "$user" >/dev/null 2>&1; then
    userdel "$user"
  fi
  as_owner rm -f "$env_file"
  echo "Removed the $service service, the $user user and $env_file"
  exit 0
fi
[ $# = 0 ] || die "unknown argument: $1 (the only option is --remove)"

if [ ! -x /usr/sbin/sshd ]; then
  apt-get update -qq
  apt-get install -y -qq openssh-server
fi
/usr/sbin/sshd -V 2>&1 | head -n 1

# Reuse what an earlier run set up while it's all still there and matches; otherwise start over.
password=""
sha=""
if [ -f "$root_dir/file.bin" ] && id "$user" >/dev/null 2>&1 &&
  existing="$(as_owner cat "$env_file" 2>/dev/null)"; then
  IFS='|' read -r _ old_user old_password old_path old_sha <<<"${existing#*=}"
  if [ "$old_user" = "$user" ] && [ "$old_path" = /file.bin ] &&
    [ "$(sha256sum "$root_dir/file.bin" | cut -d ' ' -f 1)" = "$old_sha" ]; then
    password="$old_password"
    sha="$old_sha"
  fi
fi

if [ -z "$password" ]; then
  password="$(head -c 24 /dev/urandom | base64 | tr -d '/+=')"
  if ! id "$user" >/dev/null 2>&1; then
    useradd --system --no-create-home --home-dir / --shell /usr/sbin/nologin "$user"
  fi
  echo "$user:$password" | chpasswd
  # ChrootDirectory needs a folder that only root can write to.
  install -d -o root -g root -m 755 "$root_dir"
  head -c 300000 /dev/urandom >"$root_dir/file.bin"
  chown root:"$user" "$root_dir/file.bin"
  chmod 640 "$root_dir/file.bin"
  sha="$(sha256sum "$root_dir/file.bin" | cut -d ' ' -f 1)"
  as_owner mkdir -p -m 700 "$env_dir"
  printf 'VELOCITY_SFTP_PASSWORD_SERVER=127.0.0.1:%s|%s|%s|/file.bin|%s\n' \
    "$port" "$user" "$password" "$sha" |
    as_owner sh -c 'umask 077 && cat >"$1" && chmod 600 "$1"' sh "$env_file"
  echo "Created the $user user, its file and $env_file"
fi

install -d -o root -g root -m 755 "$conf_dir"
if [ ! -f "$conf_dir/host_key" ]; then
  ssh-keygen -q -t ed25519 -N '' -C velocity-v2-sftp -f "$conf_dir/host_key"
fi
config="$(
  cat <<EOF
# Written by scripts/local-sftp-server.sh (#176). A test server: localhost, one user, SFTP only.
Port $port
ListenAddress 127.0.0.1
HostKey $conf_dir/host_key
PidFile /run/$service.pid
PasswordAuthentication yes
KbdInteractiveAuthentication no
PubkeyAuthentication no
PermitRootLogin no
UsePAM yes
AllowUsers $user
AllowTcpForwarding no
AllowAgentForwarding no
X11Forwarding no
PermitTTY no
ChrootDirectory $root_dir
Subsystem sftp internal-sftp
ForceCommand internal-sftp
EOF
)"
unit_text="$(
  cat <<EOF
# Written by scripts/local-sftp-server.sh (#176).
[Unit]
Description=Velocity V2 password-login SFTP test server (127.0.0.1:$port)
After=network.target

[Service]
ExecStartPre=/usr/bin/mkdir -p /run/sshd
ExecStartPre=/usr/sbin/sshd -t -f $conf_dir/sshd_config
ExecStart=/usr/sbin/sshd -D -f $conf_dir/sshd_config
Restart=on-failure

[Install]
WantedBy=multi-user.target
EOF
)"
changed=no
if [ "$(cat "$conf_dir/sshd_config" 2>/dev/null)" != "$config" ]; then
  printf '%s\n' "$config" >"$conf_dir/sshd_config"
  changed=yes
fi
/usr/sbin/sshd -t -f "$conf_dir/sshd_config"
if [ "$(cat "$unit" 2>/dev/null)" != "$unit_text" ]; then
  printf '%s\n' "$unit_text" >"$unit"
  systemctl daemon-reload
  changed=yes
fi
systemctl enable --quiet "$service"
if [ "$changed" = yes ]; then
  systemctl restart "$service"
else
  systemctl start "$service"
fi

for _ in $(seq 100); do
  if (exec 3<>/dev/tcp/127.0.0.1/$port) 2>/dev/null; then
    echo "SFTP password-login server ready on 127.0.0.1:$port, described in $env_file"
    exit 0
  fi
  sleep 0.1
done
systemctl status --no-pager "$service" >&2 || true
die "the $service service didn't listen on 127.0.0.1:$port"
