//! `sessio-focus`: jump from a Spotlight launcher to an open Claude Code session's Ghostty pane.
//! macOS only; set up with `sessions spotlight install`.
//!
//! One binary, two roles, picked by where it runs from:
//! - **Launcher** (`~/Sessions/<project> — <title>.app/Contents/MacOS/sessio-focus`): reads the
//!   session id from its bundle's Info.plist and hands it to the helper over a Unix socket
//!   (`<claude-home>/.sessio/focus.sock`). No `open` URL hop, no shell. Each launcher holds its
//!   own copy: LaunchServices treats bundles sharing one executable file as the same app, and
//!   opening a second one then fails with -600.
//! - **Helper** (`~/Applications/Sessio Focus.app`, kept running by launchd as `com.sessio.focus`):
//!   finds the session's current tty from Claude Code's registry (`<claude-home>/sessions/<pid>.json`,
//!   the pid's start time checked against `procStart` so a recycled pid is not trusted), and asks
//!   Ghostty to focus that pane in-process. One app talks to Ghostty, so macOS asks for Automation
//!   permission once, not once per launcher. Launchers can't start it themselves: an `open` from a
//!   launcher is held until the launcher has finished launching.
//!
//! `sessio-focus <session-id>` from a shell focuses directly, for testing.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("sessio-focus: macOS only");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn main() {
    focus::main()
}

#[cfg(target_os = "macos")]
mod focus {
    use std::ffi::{c_void, CStr, CString};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::{Duration, Instant};

    use objc2::rc::autoreleasepool;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::msg_send;

    #[link(name = "Foundation", kind = "framework")]
    extern "C" {}

    /// The launchd label of the helper; `sessions spotlight install` writes the agent.
    const LABEL: &str = "com.sessio.focus";

    fn home() -> PathBuf {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
    }

    /// `~/.claude`, honouring `CLAUDE_CONFIG_DIR` like the rest of sessio.
    fn claude_dir() -> PathBuf {
        std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home().join(".claude"))
    }

    fn socket_path() -> PathBuf {
        claude_dir().join(".sessio/focus.sock")
    }

    fn log(msg: &str) {
        let now = unsafe {
            let t = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            libc::localtime_r(&t, &mut tm);
            format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
        };
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(claude_dir().join(".sessio/focus.log"))
        {
            let _ = writeln!(f, "{now} {msg}");
        }
    }

    /// The string value right after `<key>{key}</key>` in an XML plist.
    fn plist_string(xml: &str, key: &str) -> Option<String> {
        let at = xml.find(&format!("<key>{key}</key>"))?;
        let rest = &xml[at..];
        let start = rest.find("<string>")? + "<string>".len();
        let end = rest[start..].find("</string>")?;
        Some(rest[start..start + end].to_string())
    }

    /// `.../X.app/Contents/MacOS/sessio-focus` → the session id in `.../X.app/Contents/Info.plist`.
    fn launcher_session_id() -> Option<String> {
        let exe = std::env::current_exe().ok()?;
        let plist = exe.parent()?.parent()?.join("Info.plist");
        plist_string(&std::fs::read_to_string(plist).ok()?, "SessionID")
    }

    // ---------------------------------------------------------------- launcher

    fn ask_helper(id: &str) -> std::io::Result<String> {
        let mut s = UnixStream::connect(socket_path())?;
        s.set_read_timeout(Some(Duration::from_secs(5)))?;
        writeln!(s, "{id}")?;
        let mut reply = String::new();
        BufReader::new(s).read_line(&mut reply)?;
        Ok(reply.trim().to_string())
    }

    fn run_launcher(id: &str) {
        let reply = match ask_helper(id) {
            Ok(r) => r,
            Err(_) => {
                // Helper not running. launchd keeps it alive (~/Library/LaunchAgents/com.sessio.focus.plist);
                // kick it in case it is backing off. `open`ing the app from here doesn't work: macOS
                // holds that launch until this launcher has finished launching.
                let uid = unsafe { libc::getuid() };
                let _ = Command::new("/bin/launchctl")
                    .args(["kickstart", &format!("gui/{uid}/{LABEL}")])
                    .output();
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    match ask_helper(id) {
                        Ok(r) => break r,
                        Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                        Err(e) => break format!("helper unreachable: {e}"),
                    }
                }
            }
        };
        log(&format!("launcher {} -> {reply}", &id[..id.len().min(8)]));
    }

    // ---------------------------------------------------------------- helper

    /// A string value in a Claude Code registry file, e.g. `"procStart"`.
    fn registry_string<'a>(json: &'a str, key: &str) -> Option<&'a str> {
        let quoted = format!("\"{key}\"");
        let at = json.find(&quoted)? + quoted.len();
        let rest = json[at..].trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
        Some(&rest[..rest.find('"')?])
    }

    fn bsd_info(pid: i32) -> Option<libc::proc_bsdinfo> {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let n = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut c_void, size)
        };
        (n == size).then_some(info)
    }

    /// `procStart` is the process start in UTC, ctime style ("Wed Oct 28 16:05:57 2026").
    /// Comparing it to the kernel's start time catches a pid recycled since the file was written.
    /// The day may be space- or zero-padded ("Oct  8" / "Oct 08"), so both are accepted.
    fn started_at_matches(info: &libc::proc_bsdinfo, proc_start: &str) -> bool {
        let t = info.pbi_start_tvsec as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::gmtime_r(&t, &mut tm) };
        let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let want = norm(proc_start);
        [c"%a %b %e %H:%M:%S %Y", c"%a %b %d %H:%M:%S %Y"].iter().any(|fmt| {
            let mut buf = [0 as libc::c_char; 64];
            let n = unsafe { libc::strftime(buf.as_mut_ptr(), buf.len(), fmt.as_ptr(), &tm) };
            n > 0 && norm(&unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy()) == want
        })
    }

    /// Controlling terminal of the process as `/dev/ttysNNN`, if it has one.
    fn info_tty(info: &libc::proc_bsdinfo) -> Option<String> {
        if info.e_tdev == u32::MAX || info.e_tdev == 0 {
            return None;
        }
        let dev = info.e_tdev as libc::dev_t;
        // Pseudo-terminal slaves are /dev/ttysNNN with NNN the minor number; check it really is.
        let path = format!("/dev/ttys{:03}", dev & 0xffffff);
        let c = CString::new(path.clone()).ok()?;
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        (unsafe { libc::stat(c.as_ptr(), &mut st) } == 0 && st.st_rdev == dev).then_some(path)
    }

    /// Where session `id` runs right now: (pid, tty) of the `claude` that registered it.
    fn find_session(id: &str) -> Option<(i32, String)> {
        for entry in std::fs::read_dir(claude_dir().join("sessions")).ok()?.flatten() {
            let name = entry.file_name();
            let Some(pid) = name.to_str().and_then(|n| n.strip_suffix(".json")).and_then(|p| p.parse::<i32>().ok())
            else {
                continue;
            };
            let Ok(json) = std::fs::read_to_string(entry.path()) else { continue };
            if registry_string(&json, "sessionId") != Some(id) {
                continue;
            }
            // A file left by a crashed claude can name a pid now used by something else.
            let Some(info) = bsd_info(pid) else { continue };
            if registry_string(&json, "procStart").is_some_and(|p| !started_at_matches(&info, p)) {
                continue;
            }
            if let Some(tty) = info_tty(&info) {
                return Some((pid, tty));
            }
        }
        None
    }

    /// Run AppleScript in-process (no osascript): Ghostty's `focus` raises only that pane's window
    /// and makes Ghostty frontmost; `activate` would bring every Ghostty window forward.
    fn ghostty_focus(tty: &str) -> Result<(), String> {
        // The timeout keeps a hung Ghostty from wedging the single-threaded helper (default is 2 min).
        let src = format!(
            "with timeout of 3 seconds\ntell application \"Ghostty\" to focus (first terminal whose tty is \"{tty}\")\nend timeout"
        );
        autoreleasepool(|_| unsafe {
            let src = CString::new(src).map_err(|e| e.to_string())?;
            let ns_string = AnyClass::get(c"NSString").ok_or("no NSString")?;
            let ns_src: *mut AnyObject = msg_send![ns_string, stringWithUTF8String: src.as_ptr()];
            let cls = AnyClass::get(c"NSAppleScript").ok_or("no NSAppleScript")?;
            let script: *mut AnyObject = msg_send![cls, alloc];
            let script: *mut AnyObject = msg_send![script, initWithSource: ns_src];
            let mut err: *mut AnyObject = std::ptr::null_mut();
            let result: *mut AnyObject = msg_send![script, executeAndReturnError: &mut err];
            let outcome = if result.is_null() {
                let desc: *mut AnyObject = if err.is_null() { std::ptr::null_mut() } else { msg_send![err, description] };
                let p: *const libc::c_char = if desc.is_null() { std::ptr::null() } else { msg_send![desc, UTF8String] };
                Err(if p.is_null() {
                    "unknown AppleScript error".to_string()
                } else {
                    CStr::from_ptr(p).to_string_lossy().into_owned()
                })
            } else {
                Ok(())
            };
            let _: () = msg_send![script, release];
            outcome
        })
    }

    fn handle(id: &str) -> String {
        match find_session(id) {
            None => {
                log(&format!("helper {} not open", &id[..id.len().min(8)]));
                "not open".into()
            }
            Some((_, tty)) => match ghostty_focus(&tty) {
                Ok(()) => format!("ok {tty}"),
                Err(e) => {
                    log(&format!("helper focus {tty} failed: {e}"));
                    format!("failed {tty}")
                }
            },
        }
    }

    fn run_helper() {
        let path = socket_path();
        // A live helper answers; a dead one left its socket file behind.
        if UnixStream::connect(&path).is_ok() {
            return;
        }
        let _ = std::fs::remove_file(&path);
        let listener = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) => {
                log(&format!("helper can't bind {}: {e}", path.display()));
                return;
            }
        };
        // `sessions spotlight install` replaces this app's executable; a helper still running the old
        // build answers the request in hand, then exits and launchd starts the new one.
        let installed = std::env::current_exe().unwrap_or_default();
        let inode = |p: &PathBuf| std::fs::metadata(p).ok().map(|m| std::os::unix::fs::MetadataExt::ino(&m));
        let started_with = inode(&installed);
        for conn in listener.incoming().flatten() {
            let _ = conn.set_read_timeout(Some(Duration::from_secs(2)));
            let mut reader = BufReader::new(&conn);
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let id = line.trim();
            let reply = if id.is_empty() { "empty".into() } else { handle(id) };
            let _ = (&conn).write_all(format!("{reply}\n").as_bytes());
            drop(conn);
            if inode(&installed) != started_with {
                log("helper: sessio-focus was updated, exiting so the next launch starts the new build");
                let _ = std::fs::remove_file(&path);
                return;
            }
        }
    }

    pub fn main() {
        match launcher_session_id() {
            Some(id) => run_launcher(&id),
            None => {
                // `sessio-focus <id>` from a shell focuses directly (for testing); otherwise serve.
                if let Some(id) = std::env::args().nth(1) {
                    println!("{}", handle(&id));
                } else {
                    run_helper();
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn reads_plist_string() {
            let xml = "<dict>\n\t<key>SessionID</key>\n\t<string>abc-123</string>\n</dict>";
            assert_eq!(plist_string(xml, "SessionID").as_deref(), Some("abc-123"));
            assert_eq!(plist_string(xml, "Missing"), None);
        }

        #[test]
        fn reads_registry_strings() {
            assert_eq!(registry_string(r#"{"pid":1,"sessionId":"s-1","x":2}"#, "sessionId"), Some("s-1"));
            assert_eq!(registry_string(r#"{"sessionId" : "s-2"}"#, "sessionId"), Some("s-2"));
            assert_eq!(registry_string(r#"{"bridgeSessionId":"b"}"#, "sessionId"), None);
        }

        #[test]
        fn matches_own_start_time() {
            let me = std::process::id() as i32;
            let info = bsd_info(me).expect("own bsd info");
            let t = info.pbi_start_tvsec as libc::time_t;
            let mut tm: libc::tm = unsafe { std::mem::zeroed() };
            let mut buf = [0 as libc::c_char; 64];
            unsafe {
                libc::gmtime_r(&t, &mut tm);
                libc::strftime(buf.as_mut_ptr(), buf.len(), c"%a %b %d %H:%M:%S %Y".as_ptr(), &tm);
            }
            let s = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned();
            assert!(started_at_matches(&info, &s), "{s}");
            // Same instant, day space-padded instead of zero-padded.
            let parts: Vec<&str> = s.split(' ').collect();
            let spaced = format!("{} {} {:>2} {} {}", parts[0], parts[1], parts[2].trim_start_matches('0'), parts[3], parts[4]);
            assert!(started_at_matches(&info, &spaced), "{spaced}");
            assert!(!started_at_matches(&info, "Thu Jan  1 00:00:00 1970"));
        }
    }
}
