"""Build and test Windows code under Wine; run in the disposable Wine container."""

import os
from pathlib import Path
import shutil
import subprocess
import sys


def run(*args, **kwargs):
    subprocess.run(args, check=True, timeout=600, **kwargs)


target = "x86_64-pc-windows-gnu"
if os.environ.get("SHUI_TEST_TOOLCHAIN"):
    os.environ["RUSTUP_TOOLCHAIN"] = os.environ["SHUI_TEST_TOOLCHAIN"]
run("rustup", "target", "add", target)
run(
    "cargo",
    "clippy",
    "--locked",
    "--target",
    target,
    "--all-targets",
    "--",
    "-D",
    "warnings",
)
run("cargo", "build", "--locked", "--target", target)
binary = Path(os.environ["CARGO_TARGET_DIR"]) / target / "debug/shui.exe"
# Wine replaces USERPROFILE/USERNAME during startup. Restore test values inside
# Windows, without a command shell interpreting the program's arguments.
source = Path(os.environ["CARGO_TARGET_DIR"]) / "wine-runner.c"
source.write_text(r"""
#include <windows.h>
#include <stdlib.h>
#include <wchar.h>
int wmain(void) {
    /* PowerShell needs a real console even when all three streams are pipes. */
    HANDLE input = GetStdHandle(STD_INPUT_HANDLE);
    HANDLE output = GetStdHandle(STD_OUTPUT_HANDLE);
    HANDLE error = GetStdHandle(STD_ERROR_HANDLE);
    FreeConsole();
    if (!AllocConsole()) return 4;
    SetStdHandle(STD_INPUT_HANDLE, input);
    SetStdHandle(STD_OUTPUT_HANDLE, output);
    SetStdHandle(STD_ERROR_HANDLE, error);
    const wchar_t *home = _wgetenv(L"SHUI_TEST_USERPROFILE");
    const wchar_t *user = _wgetenv(L"SHUI_TEST_USERNAME");
    if ((home && _wputenv_s(L"USERPROFILE", home)) ||
        (user && _wputenv_s(L"USERNAME", user))) return 3;
    /* Keep Wine's original Windows quoting for every forwarded argument. */
    wchar_t *command = GetCommandLineW();
    int quoted = 0;
    while (*command) {
        if (*command == L'"') quoted = !quoted;
        if (!quoted && (*command == L' ' || *command == L'\t')) break;
        command++;
    }
    while (*command == L' ' || *command == L'\t') command++;
    if (!*command) return 2;
    STARTUPINFOW startup = {0};
    PROCESS_INFORMATION process = {0};
    startup.cb = sizeof(startup);
    if (!CreateProcessW(NULL, command, NULL, NULL, TRUE, 0, NULL, NULL,
                        &startup, &process)) return 127;
    CloseHandle(process.hThread);
    DWORD status = 1;
    if (WaitForSingleObject(process.hProcess, INFINITE) == WAIT_OBJECT_0)
        GetExitCodeProcess(process.hProcess, &status);
    CloseHandle(process.hProcess);
    return (int)status;
}
""")
runner = source.with_suffix(".exe")
run(
    "x86_64-w64-mingw32-gcc",
    "-municode",
    "-Wall",
    "-Wextra",
    "-Werror",
    str(source),
    "-o",
    str(runner),
)
native_client = Path(os.environ["CARGO_TARGET_DIR"]) / "native-client.exe"
run("rustc", "--target", target, "tests/native_client.rs", "-o", str(native_client))
Path(os.environ["WINEPREFIX"]).mkdir(parents=True, exist_ok=True)
try:
    run("wineboot", "-u")
    system = Path(os.environ["WINEPREFIX"]) / "drive_c/windows/system32"
    shutil.copytree(
        "/tools/powershell", system / "WindowsPowerShell/v1.0", dirs_exist_ok=True
    )
    shutil.copyfile("/tools/sh.exe", system / "tar.exe")
    probe = subprocess.run(
        [
            "wine",
            str(runner),
            "powershell.exe",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::WriteLine('PowerShell ' + $PSVersionTable.PSVersion); exit 23",
        ],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert probe.returncode == 23 and probe.stdout.strip() == "PowerShell 7.6.9", probe
    print(probe.stdout.strip(), flush=True)
    run(
        "cargo",
        "test",
        "--locked",
        "--target",
        target,
        env={
            **os.environ,
            "CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER": f"wine {runner}",
            "SHUI_TEST_NATIVE_CLIENT": "Z:" + str(native_client).replace("/", "\\"),
        },
    )
    run(
        sys.executable,
        "tests/cli.py",
        str(binary),
        env={
            **os.environ,
            "SHUI_TEST_WINE": "wine",
            "SHUI_TEST_WINE_RUNNER": str(runner),
        },
    )
finally:
    run("wineserver", "-k")
