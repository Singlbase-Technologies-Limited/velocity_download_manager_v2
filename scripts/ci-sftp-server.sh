#!/usr/bin/env bash
# CI only, on Linux runners (#53): what the SFTP tests need and can't start themselves.
#
# - OpenSSH's sshd, installed if the image lacks it. The tests start their own unprivileged sshd
#   on free ports (crates/velocity-test-http/src/ssh.rs); VELOCITY_REQUIRE_SSHD=1 makes a missing
#   one fail instead of passing as NOT RUN.
# - A password-login SFTP server, which only root can run: a throwaway user with a random
#   password (masked in the log), an sshd as root on 127.0.0.1:2222 that serves that user only,
#   with internal-sftp and password logins only, and a file in its home folder.
#   VELOCITY_SFTP_PASSWORD_SERVER describes it to the tests, and VELOCITY_REQUIRE_SFTP_PASSWORD=1
#   makes its absence fail.
#
# The runner has passwordless sudo. Everything here ends with the runner.
set -euo pipefail

if [ ! -x /usr/sbin/sshd ]; then
  sudo apt-get update -qq
  sudo apt-get install -y -qq openssh-server
fi
/usr/sbin/sshd -V 2>&1 | head -n 1

user=v2sftp
password="$(head -c 24 /dev/urandom | base64 | tr -d '/+=')"
echo "::add-mask::$password"
if ! id "$user" >/dev/null 2>&1; then
  sudo useradd --create-home --shell /usr/sbin/nologin "$user"
fi
echo "$user:$password" | sudo chpasswd

dir="$(mktemp -d)"
sudo ssh-keygen -q -t ed25519 -N '' -C velocity-ci -f "$dir/host_key"
head -c 300000 /dev/urandom | sudo tee "/home/$user/file.bin" >/dev/null
sha="$(sudo sha256sum "/home/$user/file.bin" | cut -d ' ' -f 1)"

cat > "$dir/sshd_config" <<EOF
Port 2222
ListenAddress 127.0.0.1
HostKey $dir/host_key
PidFile $dir/sshd.pid
PasswordAuthentication yes
KbdInteractiveAuthentication no
PubkeyAuthentication no
UsePAM yes
AllowUsers $user
Subsystem sftp internal-sftp
ForceCommand internal-sftp
EOF
sudo mkdir -p /run/sshd
sudo /usr/sbin/sshd -f "$dir/sshd_config" -E "$dir/sshd.log"

for _ in $(seq 100); do
  if (exec 3<>/dev/tcp/127.0.0.1/2222) 2>/dev/null; then
    break
  fi
  sleep 0.1
done
(exec 3<>/dev/tcp/127.0.0.1/2222) 2>/dev/null || {
  echo "the password-login sshd didn't listen on 127.0.0.1:2222" >&2
  sudo cat "$dir/sshd.log" >&2
  exit 1
}

{
  echo "VELOCITY_REQUIRE_SSHD=1"
  echo "VELOCITY_REQUIRE_SFTP_PASSWORD=1"
  echo "VELOCITY_SFTP_PASSWORD_SERVER=127.0.0.1:2222|$user|$password|/home/$user/file.bin|$sha"
} >> "$GITHUB_ENV"
echo "SFTP servers ready: sshd for the tests, and a password-login server on 127.0.0.1:2222"
