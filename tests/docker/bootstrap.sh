#!/bin/sh
# Test prerequisites only. tmux, btop and Zellij must be installed by the profiles.
set -eu
# Optional trusted proxy CAs. Never disable TLS or repository signature checks.
have_proxy_ca=0
for cert in /tests/ca/*.crt; do
  [ -f "$cert" ] || continue
  have_proxy_ca=1
  case "$1" in
    fedora|rocky) anchors=/etc/pki/ca-trust/source/anchors; bundle=/etc/pki/tls/certs/ca-bundle.crt ;;
    opensuse) anchors=/etc/pki/trust/anchors; bundle=/etc/ssl/ca-bundle.pem ;;
    *) anchors=/usr/local/share/ca-certificates; bundle=/etc/ssl/certs/ca-certificates.crt ;;
  esac
  mkdir -p "$anchors" "${bundle%/*}"
  cp "$cert" "$anchors/shui-${cert##*/}"
  printf "\n" >> "$bundle"
  cat "$cert" >> "$bundle"
  printf "\n" >> "$bundle"
done
case "$1" in
  debian|ubuntu)
    export DEBIAN_FRONTEND=noninteractive
    # Preserve upstream signed-repository defaults on ordinary Docker hosts.
    # HTTPS-only proxies need trusted CAs before ca-certificates can be installed.
    if [ "$have_proxy_ca" = 1 ]; then
      for file in /etc/apt/sources.list /etc/apt/sources.list.d/*.sources; do
        [ ! -f "$file" ] || sed -i 's|http://|https://|g' "$file"
      done
      printf 'Acquire::https::CaInfo "/etc/ssl/certs/ca-certificates.crt";\n' > /etc/apt/apt.conf.d/99shui-test-ca
    fi
    apt-get update
    apt-get install -y --no-install-recommends python3 openssh-client openssh-server sudo ca-certificates curl tar gzip diffutils ncurses-bin ncurses-term procps passwd
    rm -f /etc/apt/apt.conf.d/99shui-test-ca
    ;;
  fedora)
    dnf install -y --allowerasing python3 openssh-clients openssh-server sudo ca-certificates curl tar gzip diffutils ncurses procps-ng shadow-utils passwd
    ;;
  rocky)
    # btop is supplied by EPEL on Enterprise Linux, not the base repositories.
    dnf install -y epel-release
    dnf install -y --allowerasing python3 openssh-clients openssh-server sudo ca-certificates curl tar gzip diffutils ncurses procps-ng shadow-utils passwd
    ;;
  alpine)
    apk add --no-cache python3 openssh sudo ca-certificates curl tar gzip diffutils ncurses ncurses-terminfo-base procps shadow
    ;;
  arch)
    pacman -Syu --noconfirm --needed python openssh sudo ca-certificates curl tar gzip diffutils ncurses procps-ng shadow
    ;;
  opensuse)
    zypper --non-interactive refresh
    zypper --non-interactive install python3 openssh sudo ca-certificates curl tar gzip diffutils gawk ncurses-utils ncurses-devel ncurses procps shadow
    # Tumbleweed's tmux uses /run/tmux, normally created by systemd-tmpfiles.
    # The test container has no init system, so supply the shared parent only.
    mkdir -p /run/tmux
    chmod 1777 /run/tmux
    ;;
  *) echo "Unknown distro: $1" >&2; exit 1 ;;
esac
# Minimal images do not consistently include these POSIX/ncurses utilities.
for tool in awk cmp tic infocmp; do
  command -v "$tool" >/dev/null || { echo "Missing test prerequisite: $tool" >&2; exit 1; }
done
mkdir -p /run/sshd /results /root/.ssh
chmod 700 /root/.ssh
useradd -m -U -s /bin/sh remote
printf 'remote:shui-test-password\n' | chpasswd
printf 'Defaults env_keep += "http_proxy https_proxy no_proxy HTTP_PROXY HTTPS_PROXY NO_PROXY"\nremote ALL=(ALL) NOPASSWD: ALL\n' > /etc/sudoers.d/shui-test
chmod 440 /etc/sudoers.d/shui-test
# Some minimal images ship the vendor sudoers file without its required mode.
# Establish a valid sudo baseline before exercising configuration recipes.
for file in /etc/sudoers /usr/etc/sudoers; do
  [ ! -f "$file" ] || chmod 440 "$file"
done
visudo -c
