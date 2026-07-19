//! Timestamped session logging.
//!
//! Historically this project relied on the caller piping `cargo run` through a `PowerShell`
//! `ForEach-Object` wrapper to get timestamps on every line, e.g.:
//! `cargo run | ForEach-Object { "$(Get-Date -Format '[...]') $_" }`.
//!
//! That works for non-interactive runs (`update-caches`, etc.) but breaks interactive
//! sessions: `PowerShell`'s pipeline capture of a child process's stdout is line-buffered on
//! *its* end, so any `print!` prompt without a trailing newline (used deliberately so your
//! typed answer lands on the same line) sits invisible in the pipe until some later
//! `println!` finally supplies a newline. You end up answering prompts blind.
//!
//! This module replaces that wrapper: it writes a timestamped copy of everything printed to
//! `logs/session_<timestamp>.log` *from inside the process itself*, while still writing to
//! the real stdout/stderr exactly as before (so interactive prompts stay live and immediate).
//! Run `cargo run` directly in a normal terminal — no piping needed — and check the `logs/`
//! directory afterward to see exactly what happened and when, including where things hung.
//!
//! # Usage
//! `tsprintln!`, `tsprint!`, and `tseprintln!` are drop-in equivalents of `println!`/
//! `print!`/`eprintln!` — same argument syntax — that also mirror into the log file. (An
//! earlier version of this tried to literally shadow `println!`/`print!`/`eprintln!` via
//! `use crate::{println, ...};` per-file. That doesn't work: `#[macro_export]` macros are
//! also inserted at the crate root's *textual* scope, so a `use` importing the very same
//! path in that same scope collides with it directly — `error[E0255]: the name 'println' is
//! defined multiple times` — and in non-root modules it instead collides with the std
//! prelude's `println!`, which rust-analyzer reports as an unresolvable ambiguity. Distinct
//! names sidestep both problems entirely.)
//!
//! Add `use crate::{tsprintln, tsprint, tseprintln};` (only the ones you actually use — an
//! unused macro import is just a warning) to a file's imports, then use them exactly like the
//! std macros they replace.

use chrono::Local;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};

static LOG_FILE: OnceLock<Mutex<File>> = OnceLock::new();

/// Whether the *log file* is currently positioned at the start of a line (as opposed to
/// mid-line after a `tsprint!` with no trailing newline). Used so we know whether the next
/// chunk of text needs a fresh timestamp prefix or is a continuation of the current log line.
static AT_LINE_START: Mutex<bool> = Mutex::new(true);

/// Call once, near the very top of `main()`. Creates `logs/session_<timestamp>.log` and opens
/// it for appending. Safe to call more than once (later calls are no-ops); safe to ignore
/// the error (the macros below silently fall back to plain terminal-only output if this was
/// never called or failed).
///
/// # Errors
/// Returns an error if the `logs` directory can't be created or the log file can't be opened.
pub fn init() -> std::io::Result<()> {
    std::fs::create_dir_all("logs")?;
    let filename = format!("logs/session_{}.log", Local::now().format("%Y%m%d_%H%M%S"));
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&filename)?;
    if LOG_FILE.set(Mutex::new(file)).is_ok() {
        println!("Logging this session to {filename}");
    }
    Ok(())
}

/// Splits `text` on internal newlines, writing a fresh `[timestamp]` prefix to the log file
/// only at the start of each line, and passing everything through to the file verbatim
/// otherwise. Tracks `AT_LINE_START` across calls so a `tsprint!` prompt followed later by a
/// `tsprintln!` still ends up as one continuous, correctly-timestamped log line — matching
/// what you'd see on a real terminal.
fn write_to_file(text: &str) {
    let Some(file_lock) = LOG_FILE.get() else {
        return;
    };
    let Ok(mut file) = file_lock.lock() else {
        return;
    };
    let Ok(mut at_start) = AT_LINE_START.lock() else {
        return;
    };

    let mut remaining = text;
    while !remaining.is_empty() {
        if *at_start {
            let ts = Local::now().format("[%Y-%m-%d %H:%M:%S]");
            let _ = write!(file, "{ts} ");
        }
        if let Some(idx) = remaining.find('\n') {
            let _ = writeln!(file, "{}", &remaining[..idx]);
            *at_start = true;
            remaining = &remaining[idx + 1..];
        } else {
            let _ = write!(file, "{remaining}");
            *at_start = false;
            remaining = "";
        }
    }
    let _ = file.flush();
}

/// Backing implementation for `tsprintln!`/`tsprint!`: write to the real stdout exactly as
/// `print!` would (so the terminal is unaffected), then mirror into the timestamped log file.
#[doc(hidden)]
pub fn write_stdout(text: &str) {
    print!("{text}");
    let _ = std::io::stdout().flush();
    write_to_file(text);
}

/// Backing implementation for `tseprintln!`: write to real stderr, then mirror into the same
/// timestamped log file (interleaved with stdout content, same as a real terminal would show
/// it).
#[doc(hidden)]
pub fn write_stderr(text: &str) {
    eprint!("{text}");
    let _ = std::io::stderr().flush();
    write_to_file(text);
}

/// Drop-in equivalent of `println!` that also mirrors into the timestamped session log.
#[macro_export]
macro_rules! tsprintln {
    () => {{ $crate::logging::write_stdout("\n"); }};
    ($($arg:tt)*) => {{
        let mut s = format!($($arg)*);
        s.push('\n');
        $crate::logging::write_stdout(&s);
    }};
}

/// Drop-in equivalent of `print!` that also mirrors into the timestamped session log.
#[macro_export]
macro_rules! tsprint {
    ($($arg:tt)*) => {{
        $crate::logging::write_stdout(&format!($($arg)*));
    }};
}

/// Drop-in equivalent of `eprintln!` that also mirrors into the timestamped session log.
#[macro_export]
macro_rules! tseprintln {
    () => {{ $crate::logging::write_stderr("\n"); }};
    ($($arg:tt)*) => {{
        let mut s = format!($($arg)*);
        s.push('\n');
        $crate::logging::write_stderr(&s);
    }};
}
