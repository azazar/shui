ARG BASE_IMAGE=debian:13
FROM ${BASE_IMAGE}
ARG DISTRO
ARG SERVICE_TESTS=0
COPY ca /tests/ca
COPY bootstrap.sh /tests/bootstrap.sh
RUN sh /tests/bootstrap.sh "$DISTRO"
RUN if [ "$SERVICE_TESTS" = 1 ]; then \
      case "$DISTRO" in \
        debian|ubuntu) apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y systemd systemd-sysv dbus iproute2 qemu-guest-agent libvirt-daemon-log crun ;; \
        fedora|rocky) dnf install -y systemd dbus iproute qemu-guest-agent libvirt-daemon-log ;; \
        opensuse) zypper --non-interactive install systemd dbus-1 iproute2 qemu-guest-agent libvirt-daemon-driver-qemu ;; \
        arch) pacman -S --needed --noconfirm systemd dbus iproute2 qemu-guest-agent libvirt ;; \
        *) exit 1 ;; \
      esac; \
    fi
COPY container.py /tests/container.py
COPY services.py /tests/services.py
COPY guest_logs.py /tests/guest_logs.py
COPY fixtures /tests/fixtures
ENV TERM=xterm-256color
ENV LANG=C.UTF-8
CMD ["python3", "-u", "/tests/container.py"]
