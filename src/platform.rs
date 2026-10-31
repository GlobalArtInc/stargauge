use std::io::Write;
use std::path::Path;

pub fn init_console() {
    #[cfg(windows)]
    windows::enable_virtual_terminal();
}

pub fn install_ctrlc() {
    let _ = ctrlc::set_handler(|| {
        let term = console::Term::stderr();
        let _ = term.show_cursor();
        let _ = writeln!(std::io::stderr(), "\nInterrupted");
        std::process::exit(130);
    });
}

pub fn open_report(path: &Path) -> Result<(), String> {
    if !display_available() {
        return Err("no graphical session (DISPLAY and WAYLAND_DISPLAY are not set)".into());
    }
    open::that_detached(path).map_err(|e| e.to_string())
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
fn display_available() -> bool {
    ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|v| !v.is_empty()))
}

#[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "android"))))]
fn display_available() -> bool {
    true
}

pub fn pause_if_own_console() {
    #[cfg(windows)]
    {
        if windows::console_is_ours_alone() {
            print!("press ENTER to exit");
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
        }
    }
}

pub fn local_tz_label() -> String {
    let seconds = chrono::Local::now().offset().local_minus_utc();
    let offset = format_offset(seconds);
    match iana_time_zone::get_timezone() {
        Ok(name) => format!("{name} ({offset})"),
        Err(_) => offset,
    }
}

fn format_offset(seconds: i32) -> String {
    let sign = if seconds < 0 { '-' } else { '+' };
    let minutes = seconds.unsigned_abs() / 60;
    format!("UTC{sign}{:02}:{:02}", minutes / 60, minutes % 60)
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::System::Console::{
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetConsoleProcessList, GetStdHandle,
        STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetConsoleMode,
    };

    pub fn enable_virtual_terminal() {
        for id in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let mut mode: u32 = 0;
            let handle = unsafe { GetStdHandle(id) };
            if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
                continue;
            }
            let _ = unsafe { SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) };
        }
    }

    pub fn console_is_ours_alone() -> bool {
        let mut processes = [0u32; 2];
        let count = unsafe { GetConsoleProcessList(processes.as_mut_ptr(), 2) };
        count == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_is_formatted_with_sign_hours_and_minutes() {
        assert_eq!(format_offset(3 * 3600), "UTC+03:00");
        assert_eq!(format_offset(-(5 * 3600 + 30 * 60)), "UTC-05:30");
        assert_eq!(format_offset(0), "UTC+00:00");
    }

    #[test]
    fn tz_label_ends_with_a_utc_offset() {
        let label = local_tz_label();
        assert!(label.contains("UTC+") || label.contains("UTC-"), "{label}");
    }
}
