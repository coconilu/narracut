use std::{
    fs,
    io::{self, Write as _},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("--version") => probe_version(),
        Some("exec") if args.get(1).map(String::as_str) == Some("--help") => probe_help(),
        Some("login") if args.get(1).map(String::as_str) == Some("status") => probe_login(),
        Some("child-block") => child_block(required_path(&args, 1)),
        Some("parent-block") => parent_mode(required_path(&args, 1), ParentMode::Block),
        Some("inherited-pipe-parent") => {
            parent_mode(required_path(&args, 1), ParentMode::ExitWithInheritedPipe)
        }
        Some("drain-invalid-exit") => {
            parent_mode(required_path(&args, 1), ParentMode::DrainInvalidExit)
        }
        Some("forbidden-block") => parent_mode(
            required_path(&args, 1),
            ParentMode::Forbidden {
                sentinel: required_path(&args, 2),
            },
        ),
        Some("oversize-block") => parent_mode(required_path(&args, 1), ParentMode::Oversize),
        Some("unterminated-oversize-block") => {
            parent_mode(required_path(&args, 1), ParentMode::UnterminatedOversize)
        }
        Some("success") => write_success_protocol(),
        Some("crash") => {
            write_protocol_prefix();
            std::process::exit(7);
        }
        other => panic!("unknown helper mode: {other:?}"),
    }
}

enum ParentMode {
    Block,
    ExitWithInheritedPipe,
    DrainInvalidExit,
    Forbidden { sentinel: PathBuf },
    Oversize,
    UnterminatedOversize,
}

fn parent_mode(state_file: PathBuf, mode: ParentMode) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind parent marker");
    let parent_port = listener.local_addr().expect("parent address").port();
    let child_state = state_file.with_extension("child");
    let inherited_stdout = matches!(mode, ParentMode::ExitWithInheritedPipe);
    let mut command = Command::new(std::env::current_exe().expect("helper executable"));
    command
        .arg("child-block")
        .arg(&child_state)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    if inherited_stdout {
        command.stdout(Stdio::inherit());
    } else {
        command.stdout(Stdio::null());
    }
    let child = command.spawn().expect("spawn blocking child");
    let child_marker = wait_for_text(&child_state);
    fs::write(
        &state_file,
        format!(
            "{} {}\n{}\n",
            std::process::id(),
            parent_port,
            child_marker.trim()
        ),
    )
    .expect("write process state");

    match mode {
        ParentMode::ExitWithInheritedPipe => {
            write_protocol_prefix();
            drop(child);
            drop(listener);
        }
        ParentMode::DrainInvalidExit => {
            write_protocol_prefix();
            // Keep the whole protocol tail below the adapter's bounded 32-line pump queue.
            // The test wrapper releases it only after the real process wait has completed.
            for index in 0..28 {
                println!(
                    r#"{{"type":"item.updated","item":{{"id":"reasoning-{index}","type":"reasoning"}}}}"#
                );
            }
            println!(
                r#"{{"type":"item.started","item":{{"id":"tool","type":"command_execution"}}}}"#
            );
            io::stdout().flush().expect("flush drain protocol");
            drop(child);
            drop(listener);
        }
        ParentMode::Forbidden { sentinel } => {
            write_protocol_prefix();
            println!(
                r#"{{"type":"item.started","item":{{"id":"tool","type":"command_execution","command":"delayed"}}}}"#
            );
            io::stdout().flush().expect("flush forbidden event");
            thread::sleep(Duration::from_secs(3));
            fs::write(sentinel, "sentinel-written").expect("write delayed sentinel");
            println!(r#"{{"type":"sentinel.after_forbidden"}}"#);
            io::stdout().flush().expect("flush sentinel");
            block_forever(listener, child);
        }
        ParentMode::Oversize => {
            io::stdout()
                .write_all(&vec![b'x'; 256 * 1024 + 1])
                .expect("write oversized line");
            io::stdout()
                .write_all(b"\n")
                .expect("terminate oversized line");
            io::stdout().flush().expect("flush oversized line");
            block_forever(listener, child);
        }
        ParentMode::UnterminatedOversize => {
            io::stdout()
                .write_all(&vec![b'x'; 512 * 1024])
                .expect("write unterminated oversized fragment");
            io::stdout()
                .flush()
                .expect("flush unterminated oversized fragment");
            block_forever(listener, child);
        }
        ParentMode::Block => block_forever(listener, child),
    }
}

fn child_block(state_file: PathBuf) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind child marker");
    let port = listener.local_addr().expect("child address").port();
    fs::write(state_file, format!("{} {}", std::process::id(), port))
        .expect("write child marker");
    loop {
        let _ = &listener;
        thread::sleep(Duration::from_secs(60));
    }
}

fn block_forever(_listener: TcpListener, _child: std::process::Child) -> ! {
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

fn write_protocol_prefix() {
    println!(r#"{{"type":"thread.started","thread_id":"helper-thread"}}"#);
    println!(r#"{{"type":"turn.started"}}"#);
    io::stdout().flush().expect("flush protocol prefix");
}

fn write_success_protocol() {
    write_protocol_prefix();
    println!(
        r#"{{"type":"item.completed","item":{{"id":"message","type":"agent_message","text":"{{}}"}}}}"#
    );
    println!(
        r#"{{"type":"turn.completed","usage":{{"input_tokens":1,"output_tokens":1}}}}"#
    );
    io::stdout().flush().expect("flush success protocol");
}

fn probe_version() {
    let version = config_value("version").unwrap_or_else(|| "0.144.1".to_owned());
    if version == "fail" {
        std::process::exit(9);
    }
    println!("codex-cli {version}");
}

fn probe_help() {
    if config_value("help").as_deref() == Some("missing") {
        println!("--json --ephemeral");
        return;
    }
    println!(
        "--json --ephemeral --ignore-user-config --ignore-rules --sandbox --color \
         --skip-git-repo-check --model --output-schema --cd"
    );
}

fn probe_login() {
    if config_value("login").as_deref() == Some("missing") {
        std::process::exit(1);
    }
    println!("Logged in");
}

fn config_value(key: &str) -> Option<String> {
    let config = std::env::current_exe().ok()?.with_extension("config");
    fs::read_to_string(config)
        .ok()?
        .lines()
        .filter_map(|line| line.split_once('='))
        .find_map(|(candidate, value)| (candidate.trim() == key).then(|| value.trim().to_owned()))
}

fn wait_for_text(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(value) = fs::read_to_string(path) {
            if !value.trim().is_empty() {
                return value;
            }
        }
        assert!(Instant::now() < deadline, "child marker timeout");
        thread::sleep(Duration::from_millis(10));
    }
}

fn required_path(args: &[String], index: usize) -> PathBuf {
    args.get(index)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("missing path argument {index}"))
}
