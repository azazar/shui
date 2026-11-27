//! Dependency-free native client fixture for Windows/Wine integration tests.
use std::{
    env,
    fs::{self, OpenOptions},
    io::{Read, Write},
};
fn main() {
    let args: Vec<_> = env::args().collect();
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).unwrap();
    let input = if args.iter().any(|arg| arg.contains("tar -xf -")) {
        input
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    } else {
        String::from_utf8(input).unwrap()
    };
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(env::var_os("SHUI_NATIVE_LOG").unwrap())
        .unwrap();
    log.write_all(&((args.len() + 1) as u32).to_le_bytes())
        .unwrap();
    for value in args.iter().chain(std::iter::once(&input)) {
        log.write_all(&(value.len() as u32).to_le_bytes()).unwrap();
        log.write_all(value.as_bytes()).unwrap();
    }
    if let Ok(code) = env::var("SHUI_NATIVE_EXIT") {
        std::process::exit(code.parse().unwrap());
    }
    let name = std::path::Path::new(&args[0])
        .file_stem()
        .unwrap()
        .to_string_lossy();
    if name == "ssh-keygen" {
        let path = &args[args.iter().position(|a| a == "-f").unwrap() + 1];
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        fs::write(format!("{path}.pub"), "ssh-ed25519 AAAATEST generated\n").unwrap();
    }
    if name == "infocmp" {
        println!("test-terminal|fixture,\n\tcols#80,");
    }
}
