& {
    $ErrorActionPreference = "Stop"
    $version = "v0.1.0-rc.11"
    $target = switch ($env:PROCESSOR_ARCHITECTURE) {
        "AMD64" { "x86_64-pc-windows-msvc" }
        "ARM64" { "aarch64-pc-windows-msvc" }
        default { throw "Use a 64-bit PowerShell on Windows x64 or ARM64." }
    }
    $archive = "shui-$target.zip"
    $base = "https://github.com/azazar/shui/releases/download/$version"
    $temporary = Join-Path ([IO.Path]::GetTempPath()) ("shui-" + [guid]::NewGuid())
    New-Item -ItemType Directory $temporary | Out-Null
    try {
        Invoke-WebRequest "$base/$archive" -OutFile "$temporary/$archive"
        Invoke-WebRequest "$base/SHA256SUMS" -OutFile "$temporary/SHA256SUMS"
        $pattern = "^[0-9a-f]{64}  " + [regex]::Escape($archive) + "$"
        $matching = @(Get-Content "$temporary/SHA256SUMS" | Where-Object { $_ -cmatch $pattern })
        if ($matching.Count -ne 1) { throw "Expected exactly one checksum for $archive." }
        $expected = $matching[0].Split(' ')[0]
        $actual = (Get-FileHash -Algorithm SHA256 "$temporary/$archive").Hash
        if ($actual -ne $expected) { throw "Checksum verification failed; nothing installed." }
        Expand-Archive "$temporary/$archive" "$temporary/unpacked"
        $destination = if ($env:SHUI_INSTALL_DIR) { $env:SHUI_INSTALL_DIR } else { "$HOME/.local/bin" }
        New-Item -ItemType Directory -Force $destination | Out-Null
        Copy-Item "$temporary/unpacked/shui.exe" "$destination/shui.exe" -Force
        Write-Host "Installed Shui $version to $destination/shui.exe"
        if (($env:PATH -split [IO.Path]::PathSeparator) -notcontains $destination) {
            Write-Host "Add $destination to your PATH, then run shui."
        } else {
            Write-Host "Run: shui"
        }
    } finally {
        Remove-Item -Recurse -Force $temporary
    }
}
