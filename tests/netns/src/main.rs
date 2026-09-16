//! Privileged integration tests against real network namespaces and real
//! kernel objects (AGENTS.md §1.2, §4; PLAN.md §11, §13).
//!
//! **Not run by `cargo test --workspace`.** This binary has no `#[test]`
//! functions on purpose, so it's never swept into the checks every
//! contributor and CI run unprivileged. Run it explicitly:
//!
//! ```text
//! cargo xtask test-netns
//! ```
//!
//! ...inside an environment that actually has `CAP_NET_ADMIN` and
//! `CAP_SYS_ADMIN` — a container started with those capabilities added, a
//! VM, or a machine you're fine restarting if something goes wrong.
//! AGENTS.md §1.2 gates even building toward this on the human asking for it
//! in session, for exactly that reason.
//!
//! Without privilege, the checks that need it report that clearly and this
//! still exits successfully — running this by accident, unprivileged,
//! should never look like a failure.

#[cfg(target_os = "linux")]
mod linux {
    use std::net::Ipv4Addr;
    use std::path::Path;
    use std::process::{Command, Stdio};

    use narrows_cni::netlink::socket::{AckError, RouteSocket};
    use narrows_cni::netlink::{addr, link, route};
    use narrows_cni::netns;

    /// One named check. `run` reports what it did via `println!`/`eprintln!`
    /// and returns `Ok` on success, `Err` with a message on failure. A
    /// check that can't run for lack of privilege should say so and still
    /// return `Ok` — that's not this run failing, it's this run telling the
    /// truth about its environment.
    struct Check {
        name: &'static str,
        run: fn() -> Result<(), String>,
    }

    const CHECKS: &[Check] = &[
        Check {
            name: "open and close a NETLINK_ROUTE socket (no privilege needed)",
            run: open_and_close_socket,
        },
        Check {
            name: "create a veth pair, verify with /sys/class/net, clean up",
            run: create_and_delete_veth_pair,
        },
        Check {
            name: "enter and leave a namespace with setns, verify via /proc/*/ns/net",
            run: enter_and_leave_a_namespace,
        },
    ];

    pub fn run() -> std::process::ExitCode {
        let mut failed = 0usize;
        for check in CHECKS {
            print!("▶ {} ... ", check.name);
            match (check.run)() {
                Ok(()) => println!("ok"),
                Err(message) => {
                    println!("FAILED");
                    eprintln!("  {message}");
                    failed += 1;
                }
            }
        }
        if failed == 0 {
            println!("\n✔ all checks passed");
            std::process::ExitCode::SUCCESS
        } else {
            eprintln!("\n✘ {failed} check(s) failed");
            std::process::ExitCode::FAILURE
        }
    }

    fn open_and_close_socket() -> Result<(), String> {
        RouteSocket::open().map(drop).map_err(|e| e.to_string())
    }

    /// Whether an `AckError` is the kernel reporting "you don't have
    /// permission," as opposed to this crate having built a bad message.
    fn is_permission_denied(error: &AckError) -> bool {
        matches!(error, AckError::Rejected(e) if e.raw_os_error() == Some(libc::EPERM))
    }

    fn create_and_delete_veth_pair() -> Result<(), String> {
        let host = "narrowstest-h";
        let peer = "narrowstest-p";
        let socket = RouteSocket::open().map_err(|e| e.to_string())?;

        let create = link::create_veth_pair(host, peer, 1);
        match socket.send_and_ack(&create) {
            Ok(()) => {}
            Err(e) if is_permission_denied(&e) => {
                return Err(format!(
                    "needs CAP_NET_ADMIN to create a link; got: {e} (this is expected \
                     without it — run inside a container started with \
                     --cap-add=NET_ADMIN --cap-add=SYS_ADMIN)"
                ));
            }
            Err(e) => return Err(format!("creating the veth pair failed: {e}")),
        }

        // From here, always try to clean up, even if a later step fails —
        // this is a shared dev/CI machine's real network namespace, and a
        // failed check shouldn't also leave stray interfaces behind.
        let result = verify_and_use_veth_pair(&socket, host, peer);
        let cleanup = cleanup_veth_pair(&socket, host);
        result.and(cleanup)
    }

    fn verify_and_use_veth_pair(
        socket: &RouteSocket,
        host: &str,
        peer: &str,
    ) -> Result<(), String> {
        if !sys_class_net_exists(host) {
            return Err(format!(
                "/sys/class/net/{host} does not exist after creating it"
            ));
        }
        if !sys_class_net_exists(peer) {
            return Err(format!(
                "/sys/class/net/{peer} does not exist after creating it"
            ));
        }

        let host_ifindex = read_ifindex(host)?;
        let peer_ifindex = read_ifindex(peer)?;

        // A freshly created veth pair starts administratively down on both
        // ends; routing through a down link fails with ENETDOWN.
        for (name, ifindex) in [(host, host_ifindex), (peer, peer_ifindex)] {
            let up = link::set_link_up(ifindex, 10 + ifindex);
            socket
                .send_and_ack(&up)
                .map_err(|e| format!("bringing {name} up failed: {e}"))?;
        }

        let pod_address = Ipv4Addr::new(169, 254, 200, 1);
        let assign = addr::add_address(pod_address, 32, peer_ifindex, 2);
        socket
            .send_and_ack(&assign)
            .map_err(|e| format!("assigning an address to {peer} failed: {e}"))?;

        let shown = Command::new("ip")
            .args(["-4", "-o", "addr", "show", "dev", peer])
            .output()
            .map_err(|e| format!("could not run `ip addr show`: {e}"))?;
        let shown = String::from_utf8_lossy(&shown.stdout);
        if !shown.contains(&pod_address.to_string()) {
            return Err(format!(
                "`ip addr show dev {peer}` doesn't mention {pod_address}: {shown:?}"
            ));
        }

        let host_route = route::add_host_route(pod_address, host_ifindex, 3);
        socket
            .send_and_ack(&host_route)
            .map_err(|e| format!("adding the host route to {pod_address} failed: {e}"))?;

        Ok(())
    }

    fn cleanup_veth_pair(socket: &RouteSocket, host: &str) -> Result<(), String> {
        let Ok(host_ifindex) = read_ifindex(host) else {
            // Already gone, or never created — nothing to clean up.
            return Ok(());
        };
        let delete = link::delete_link(host_ifindex, 4);
        socket
            .send_and_ack(&delete)
            .map_err(|e| format!("deleting {host} (ifindex {host_ifindex}) failed: {e}"))?;
        if sys_class_net_exists(host) {
            return Err(format!(
                "/sys/class/net/{host} still exists after deleting it"
            ));
        }
        Ok(())
    }

    fn sys_class_net_exists(name: &str) -> bool {
        Path::new("/sys/class/net").join(name).exists()
    }

    fn read_ifindex(name: &str) -> Result<u32, String> {
        let raw = std::fs::read_to_string(format!("/sys/class/net/{name}/ifindex"))
            .map_err(|e| format!("reading {name}'s ifindex: {e}"))?;
        raw.trim()
            .parse()
            .map_err(|e| format!("{name}'s ifindex {raw:?} isn't a number: {e}"))
    }

    /// Starts a detached process in a brand-new, empty network namespace
    /// (`unshare --net`), so [`netns::enter`] has somewhere real to switch
    /// into. `unshare` (from `util-linux`) is used instead of hand-rolling
    /// `fork`+`unshare(2)` here: this crate's own `unsafe` surface stays
    /// limited to exactly the syscalls it actually ships (`setns`, and the
    /// netlink socket calls) — `fork` has its own, much larger set of
    /// correctness rules this test doesn't need to take on.
    struct DisposableNamespace {
        child: std::process::Child,
    }

    impl DisposableNamespace {
        fn spawn() -> Result<Self, String> {
            let child = Command::new("unshare")
                .args(["--net", "sleep", "300"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("could not run `unshare --net`: {e}"))?;
            Ok(Self { child })
        }

        fn ns_path(&self) -> std::path::PathBuf {
            format!("/proc/{}/ns/net", self.child.id()).into()
        }

        /// Waits until the child has actually called `unshare(2)` and its
        /// namespace differs from ours, rather than trusting that `spawn`
        /// returning means the child has gotten that far yet — `unshare`
        /// the command still has to start up and call it before exec'ing
        /// `sleep`, and that's not synchronized with `Command::spawn`
        /// returning.
        fn wait_until_unshared(&self) -> Result<(), String> {
            let ours = std::fs::read_link("/proc/thread-self/ns/net").map_err(|e| e.to_string())?;
            for _ in 0..200 {
                if let Ok(theirs) = std::fs::read_link(self.ns_path())
                    && theirs != ours
                {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err("child never appeared to unshare its network namespace (waited 2s)".to_owned())
        }
    }

    impl Drop for DisposableNamespace {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// Verifies [`netns::enter`] by checking the calling thread's namespace
    /// *identity* (the inode `/proc/thread-self/ns/net` points at), not by
    /// checking `/sys/class/net`'s contents.
    ///
    /// An earlier version of this check read `/sys/class/net` before and
    /// after, expecting it to show the fresh, near-empty target namespace.
    /// It didn't — `/sys/class/net` kept showing this process's own
    /// interfaces even after `setns` genuinely succeeded (confirmed with the
    /// same readlink comparison used here). The reason: a sysfs mount's
    /// namespace-awareness is fixed at *mount time*, not read time.
    /// Switching a thread's namespace membership with `setns` doesn't
    /// retroactively change what an already-mounted `/sys` shows — that's
    /// exactly why `ip netns exec` also creates a fresh mount namespace and
    /// remounts `/sys` internally, rather than relying on `setns` alone.
    /// Reading `/proc/thread-self/ns/net` has no such caveat: it always
    /// reflects the calling thread's current namespace.
    fn enter_and_leave_a_namespace() -> Result<(), String> {
        let disposable = DisposableNamespace::spawn()?;
        disposable.wait_until_unshared()?;

        let original = current_ns()?;
        let target = read_ns(&disposable.ns_path())?;
        if original == target {
            return Err(format!(
                "disposable namespace ({target}) is the same as our own ({original}) — this check would prove nothing"
            ));
        }

        // SAFETY: this whole binary runs single-threaded — `main` never
        // spawns an OS thread — so the calling thread here is the only one
        // `narrows-cni` itself ever runs on, matching `enter`'s safety
        // requirement.
        let guard = unsafe { netns::enter(&disposable.ns_path()) }
            .map_err(|e| format!("entering the disposable namespace failed: {e}"))?;

        let while_entered = current_ns()?;
        drop(guard);
        let after = current_ns()?;

        if while_entered != target {
            return Err(format!(
                "while the guard was held, thread-self ns was {while_entered}, expected the target {target}"
            ));
        }
        if after != original {
            return Err(format!(
                "after the guard dropped, thread-self ns was {after}, expected the original {original}"
            ));
        }
        Ok(())
    }

    fn current_ns() -> Result<String, String> {
        read_ns(Path::new("/proc/thread-self/ns/net"))
    }

    fn read_ns(path: &Path) -> Result<String, String> {
        std::fs::read_link(path)
            .map(|target| target.to_string_lossy().into_owned())
            .map_err(|e| format!("reading {}: {e}", path.display()))
    }
}

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    linux::run()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    println!("netns-tests: only runs on Linux (uses AF_NETLINK and setns); nothing to do here.");
}
