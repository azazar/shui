ARG RUST_VERSION=1.99
FROM rust:${RUST_VERSION}-slim-trixie
RUN apt-get update && apt-get install -y --no-install-recommends \
      wine gcc-mingw-w64-x86-64-posix python3 unzip xvfb xauth \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add x86_64-pc-windows-gnu
ADD --checksum=sha256:8263abc2aee6b6f3b3c41aa704bee98987bf7b1ea9bce833f75c7bce89ec1602 https://github.com/moby/busybox/releases/download/FRP-5007-g82accfc19/busybox-w64-FRP-5007-g82accfc19.exe /tools/sh.exe
ADD --checksum=sha256:ce7484804c0f0ad4dda10c958e71acdaf7eebfb22505c1b480f89e0e9f1da94d https://github.com/PowerShell/PowerShell/releases/download/v7.6.9/PowerShell-7.6.9-win-x64.zip /tools/powershell.zip
RUN unzip -q /tools/powershell.zip -d /tools/powershell \
    && cp /tools/powershell/pwsh.exe /tools/powershell/powershell.exe \
    && rm /tools/powershell.zip
RUN mkdir -p /root/tmp /run/user/0 && chmod 700 /run/user/0
ENV CARGO_TARGET_DIR=/build WINEPREFIX=/wine WINEDEBUG=-all WINEPATH=Z:\\tools
ENV TMPDIR=/root/tmp XDG_RUNTIME_DIR=/run/user/0
WORKDIR /src
CMD ["xvfb-run", "-a", "python3", "tests/wine.py"]
