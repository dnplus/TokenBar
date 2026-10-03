fn main() {
    let mut table = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--table" => table = true,
            "--json" => table = false,
            "-h" | "--help" => {
                eprintln!("{HELP}");
                return;
            }
            other => {
                eprintln!("unknown argument {other}\n{HELP}");
                std::process::exit(2);
            }
        }
    }
    println!("{}", tb_core_ffi::quota_dump(table));
}

const HELP: &str = "\
tokenbar-quota

Print every TokenBar quota window as JSON.
Run this on the Mac where Claude, Codex, Grok, Antigravity, and Cursor are signed in.
It does not open Syrtis.

  CARGO_PROFILE_RELEASE_STRIP=none cargo build --release -p tb_core_ffi --bin tokenbar-quota
  ./target/release/tokenbar-quota
  ./target/release/tokenbar-quota --table

--json is the default. A poll that succeeds also writes one pace sample,
the same write the menu bar makes.
";
