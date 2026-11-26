//! Whether Linux or FreeBSD has a display to show the window on, checked before GPUI connects to it. Without one GPUI
//! runs headless, its window unseen and its loop never ending; with one that's named but dead it panics, after
//! libxcb's TCP fallback has waited out a connection timeout on X11. Either way the app wouldn't say why.
//!
//! What this can't tell: a host-named X display whose name takes long to resolve, and a server that accepts the
//! connection but then stalls or refuses it (X authorization). GPUI's panic then is caught by [`super::application`].

use anyhow::{Result, bail};
use std::ffi::OsString;
use std::net::{TcpStream, ToSocketAddrs};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a host-named X display (`ssh -X`'s `localhost:10`) has to answer.
const TCP_TIMEOUT: Duration = Duration::from_secs(2);

/// What to do when there's none, after what's wrong.
const RUN_FROM_DESKTOP: &str = "run rs3manager from the desktop session, not over ssh or under `sudo -u other`";

/// Where GPUI would connect to show its window.
#[derive(Debug, PartialEq)]
enum Socket {
    /// A Unix socket file: Wayland's, or an X server's in `/tmp/.X11-unix`.
    Unix(PathBuf),
    /// A Unix socket in Linux's abstract namespace, its name without the leading NUL: where an X server listens too.
    #[cfg(target_os = "linux")]
    Abstract(String),
    /// An X server on a host, at its port.
    Tcp(String, u16),
}

/// Fails, saying why and to run it from the desktop session, when there's no display to show the window on, or when
/// the one named doesn't answer: over ssh, or under `sudo -u other`, which drops `DISPLAY` and `WAYLAND_DISPLAY`
/// (plain `sudo` is refused first, as root), or with one left from a session that's gone.
pub fn ensure_display() -> Result<()> {
    ensure_display_on(gpui::guess_compositor(), |name| std::env::var_os(name), answers)
}

/// [`ensure_display`] with `compositor`, what GPUI runs on (from [`gpui::guess_compositor`]), the environment's
/// variables from `var`, and `answers` saying whether something listens at a socket.
fn ensure_display_on(
    compositor: &str,
    var: impl Fn(&str) -> Option<OsString>,
    answers: impl Fn(&Socket) -> bool,
) -> Result<()> {
    let shown = |name| var(name).unwrap_or_default().to_string_lossy().into_owned();
    match compositor {
        "Headless" | "headless" => bail!(
            "there's no display to open the window on (no WAYLAND_DISPLAY or DISPLAY): {RUN_FROM_DESKTOP}"
        ),
        // A socket handed down by a compositor that started the app is already connected.
        "Wayland" if var("WAYLAND_SOCKET").is_none() => {
            let display = shown("WAYLAND_DISPLAY");
            let Some(socket) = wayland_socket(Path::new(&display), var("XDG_RUNTIME_DIR")) else {
                bail!(
                    "WAYLAND_DISPLAY={display} names a socket in XDG_RUNTIME_DIR, which isn't set (to an absolute \
                     path): {RUN_FROM_DESKTOP}, or unset WAYLAND_DISPLAY to use X11"
                );
            };
            if !answers(&Socket::Unix(socket.clone())) {
                bail!(
                    "no Wayland compositor at WAYLAND_DISPLAY={display} (nothing answers at {}): {RUN_FROM_DESKTOP}, \
                     or unset WAYLAND_DISPLAY to use X11",
                    socket.display()
                );
            }
        }
        "X11" => {
            let display = shown("DISPLAY");
            let sockets = x11_sockets(&display);
            if !sockets.is_empty() && !sockets.iter().any(answers) {
                bail!("no X server at DISPLAY={display}: {RUN_FROM_DESKTOP}");
            }
        }
        _ => {}
    }
    Ok(())
}

/// The Wayland compositor's socket `display` names, as libwayland finds it: itself if absolute, else in
/// `runtime_dir`, which must be absolute. None without one.
fn wayland_socket(display: &Path, runtime_dir: Option<OsString>) -> Option<PathBuf> {
    if display.is_absolute() {
        return Some(display.to_path_buf());
    }
    let dir = PathBuf::from(runtime_dir?);
    dir.is_absolute().then(|| dir.join(display))
}

/// Where libxcb looks for the X server `display` names, `[host]:n[.screen]`: on this machine its socket in
/// `/tmp/.X11-unix` (on Linux the abstract one of that name too), on a host its TCP port 6000 + n. None for a name
/// this doesn't read (a protocol given, `DECnet`'s `host::n`), which is left to GPUI.
fn x11_sockets(display: &str) -> Vec<Socket> {
    let Some((host, rest)) = display.rsplit_once(':') else { return Vec::new() };
    let number = rest.split_once('.').map_or(rest, |(number, _)| number);
    let Ok(number) = number.parse::<u16>() else { return Vec::new() };
    if host.contains('/') || host.ends_with(':') {
        return Vec::new();
    }
    if host.is_empty() || host == "unix" {
        let path = format!("/tmp/.X11-unix/X{number}");
        return vec![
            #[cfg(target_os = "linux")]
            Socket::Abstract(path.clone()),
            Socket::Unix(PathBuf::from(path)),
        ];
    }
    number.checked_add(6000).map_or_default(|port| vec![Socket::Tcp(host.to_string(), port)])
}

/// Whether something accepts a connection at `socket`; closed right away, which a compositor or an X server takes
/// for a client gone.
fn answers(socket: &Socket) -> bool {
    match socket {
        Socket::Unix(path) => UnixStream::connect(path).is_ok(),
        #[cfg(target_os = "linux")]
        Socket::Abstract(name) => {
            use std::os::linux::net::SocketAddrExt;
            std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())
                .and_then(|addr| UnixStream::connect_addr(&addr))
                .is_ok()
        }
        Socket::Tcp(host, port) => (host.as_str(), *port)
            .to_socket_addrs()
            .is_ok_and(|mut addrs| addrs.any(|addr| TcpStream::connect_timeout(&addr, TCP_TIMEOUT).is_ok())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// The environment `vars` gives, as `std::env::var_os` would.
    fn env<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        let vars: HashMap<_, _> = vars.iter().copied().collect();
        move |name| vars.get(name).map(OsString::from)
    }

    /// [`ensure_display_on`] with `vars`, where only `listening` answers; its error, and the sockets it tried.
    fn check(compositor: &str, vars: &[(&str, &str)], listening: &[Socket]) -> (Option<String>, Vec<Socket>) {
        let tried = RefCell::new(Vec::new());
        let result = ensure_display_on(compositor, env(vars), |socket| {
            let answers = listening.contains(socket);
            tried.borrow_mut().push(match socket {
                Socket::Unix(path) => Socket::Unix(path.clone()),
                #[cfg(target_os = "linux")]
                Socket::Abstract(name) => Socket::Abstract(name.clone()),
                Socket::Tcp(host, port) => Socket::Tcp(host.clone(), *port),
            });
            answers
        });
        (result.err().map(|e| e.to_string()), tried.into_inner())
    }

    /// Where an X server on this machine listens for display `n`.
    fn local_x(n: u16) -> Vec<Socket> {
        let path = format!("/tmp/.X11-unix/X{n}");
        vec![
            #[cfg(target_os = "linux")]
            Socket::Abstract(path.clone()),
            Socket::Unix(PathBuf::from(path)),
        ]
    }

    #[test]
    fn without_a_display_the_app_says_to_run_it_from_the_desktop() {
        // What GPUI says itself with no display (Zed 1.29 made it lowercase: `"headless"` is taken too).
        let (error, tried) = check("Headless", &[], &[]);
        let error = error.expect("GPUI would run headless, its loop never ending");
        assert!(
            error.contains("no WAYLAND_DISPLAY or DISPLAY")
                && error.contains("run rs3manager from the desktop session")
        );
        assert_eq!(tried, Vec::new());
    }

    #[test]
    fn a_wayland_compositor_that_answers_opens_the_board() {
        let socket = Socket::Unix("/run/user/1000/wayland-1".into());
        let vars = [("WAYLAND_DISPLAY", "wayland-1"), ("XDG_RUNTIME_DIR", "/run/user/1000")];
        assert_eq!(check("Wayland", &vars, &[Socket::Unix("/run/user/1000/wayland-1".into())]), (None, vec![socket]));
        let absolute = [("WAYLAND_DISPLAY", "/run/compositor")];
        assert_eq!(check("Wayland", &absolute, &[Socket::Unix("/run/compositor".into())]).0, None, "used as it is");
    }

    #[test]
    fn a_wayland_display_nothing_answers_at_says_to_unset_it() {
        let vars = [("WAYLAND_DISPLAY", "wayland-1"), ("XDG_RUNTIME_DIR", "/run/user/1000")];
        let error = check("Wayland", &vars, &[]).0.expect("GPUI would panic");
        assert!(
            error.starts_with(
                "no Wayland compositor at WAYLAND_DISPLAY=wayland-1 (nothing answers at /run/user/1000/wayland-1)"
            ),
            "{error}"
        );
        assert!(
            error.contains("run rs3manager from the desktop session")
                && error.ends_with("unset WAYLAND_DISPLAY to use X11")
        );
    }

    #[test]
    fn a_wayland_display_without_its_runtime_dir_says_so() {
        for vars in
            [&[("WAYLAND_DISPLAY", "wayland-1")][..], &[("WAYLAND_DISPLAY", "wayland-1"), ("XDG_RUNTIME_DIR", "run")]]
        {
            let (error, tried) = check("Wayland", vars, &[]);
            let error = error.expect("libwayland finds no socket");
            assert!(
                error.starts_with("WAYLAND_DISPLAY=wayland-1 names a socket in XDG_RUNTIME_DIR, which isn't set"),
                "{error}"
            );
            assert_eq!(tried, Vec::new());
        }
    }

    #[test]
    fn a_wayland_socket_handed_down_is_already_connected() {
        let vars = [("WAYLAND_SOCKET", "3"), ("WAYLAND_DISPLAY", "wayland-1")];
        assert_eq!(check("Wayland", &vars, &[]), (None, Vec::new()));
    }

    #[test]
    fn an_x_server_on_this_machine_is_tried_at_its_sockets() {
        for display in [":0", ":0.1", "unix:0"] {
            let (error, tried) = check("X11", &[("DISPLAY", display)], &[Socket::Unix("/tmp/.X11-unix/X0".into())]);
            assert_eq!(error, None, "{display}");
            assert_eq!(tried, local_x(0), "{display}");
        }
        #[cfg(target_os = "linux")]
        assert_eq!(
            check("X11", &[("DISPLAY", ":3")], &[Socket::Abstract("/tmp/.X11-unix/X3".into())]).0,
            None,
            "abstract only"
        );
    }

    #[test]
    fn an_x_display_nothing_answers_at_says_so() {
        let (error, tried) = check("X11", &[("DISPLAY", ":99")], &[]);
        let error = error.expect("GPUI would panic, after libxcb's TCP fallback");
        assert!(error.starts_with("no X server at DISPLAY=:99: run rs3manager from the desktop session"), "{error}");
        assert_eq!(tried, local_x(99));
    }

    #[test]
    fn an_x_display_on_a_host_is_tried_at_its_port() {
        let ssh = Socket::Tcp("localhost".into(), 6010);
        assert_eq!(
            check("X11", &[("DISPLAY", "localhost:10.0")], &[Socket::Tcp("localhost".into(), 6010)]),
            (None, vec![ssh])
        );
        let error = check("X11", &[("DISPLAY", "localhost:10.0")], &[]).0.expect("nothing at 6010");
        assert!(error.starts_with("no X server at DISPLAY=localhost:10.0"), "{error}");
    }

    #[test]
    fn an_x_display_it_cant_read_is_left_to_gpui() {
        for display in ["tcp/localhost:0", "host::0", "nonsense", ":x", "host:65000"] {
            assert_eq!(check("X11", &[("DISPLAY", display)], &[]), (None, Vec::new()), "{display}");
        }
    }

    #[test]
    fn something_listening_answers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wayland-1");
        assert!(!answers(&Socket::Unix(path.clone())), "nothing there yet");
        let _unix = std::os::unix::net::UnixListener::bind(&path).unwrap();
        assert!(answers(&Socket::Unix(path)));

        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = tcp.local_addr().unwrap().port();
        assert!(answers(&Socket::Tcp("127.0.0.1".into(), port)));
        drop(tcp);
        assert!(!answers(&Socket::Tcp("127.0.0.1".into(), port)), "closed");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn something_listening_in_the_abstract_namespace_answers() {
        use std::os::linux::net::SocketAddrExt;
        let name = format!("rs3manager-test-{}", std::process::id());
        assert!(!answers(&Socket::Abstract(name.clone())), "nothing there yet");
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
        let _listener = std::os::unix::net::UnixListener::bind_addr(&addr).unwrap();
        assert!(answers(&Socket::Abstract(name)));
    }
}
