//! Guest-side 9P proxy for virtio-serial transport.
//!
//! The Linux kernel's 9P `trans=fd` transport uses `kernel_write()` to send
//! data, but the virtio_console driver does not support kernel-space writes.
//! This proxy bridges the gap: it creates a Unix socketpair (which does
//! support `kernel_write()`), passes one end to `mount -t 9p` via `trans=fd`,
//! and bidirectionally copies data between the other end and the virtio-serial
//! port device.
//!
//! The proxy forks before calling mount: the child runs the data proxy loop
//! (so 9P messages flow immediately), while the parent calls mount and then
//! exits. The child daemon runs for the lifetime of the mount.
//!
//! Usage: p9proxy <port-device> <mount-point>
//!   e.g. p9proxy /dev/virtio-ports/p9fs0 /mnt/working

#[cfg(unix)]
mod proxy {
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read, Write};
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::process::Command;
    use std::thread;

    /// Clear the close-on-exec flag so the fd survives across exec.
    fn clear_cloexec(fd: i32) {
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 {
                libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            }
        }
    }

    /// Bidirectional copy between a Unix socket and a file (the virtio port).
    /// Runs forever until the mount is unmounted or the port is closed.
    fn proxy_loop(sock: UnixStream, port: File) -> ! {
        let mut port_reader = port.try_clone().expect("clone port for read");
        let mut sock_writer = sock.try_clone().expect("clone sock for write");

        // Thread 1: port -> socket (host responses -> kernel)
        let t1 = thread::spawn(move || {
            let mut buf = [0u8; 65536];
            loop {
                match port_reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if sock_writer.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });

        // Thread 2: socket -> port (kernel requests -> host)
        let mut sock_reader = sock;
        let mut port_writer = port;
        let t2 = thread::spawn(move || {
            let mut buf = [0u8; 65536];
            loop {
                match sock_reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if port_writer.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });

        t1.join().ok();
        t2.join().ok();

        std::process::exit(0);
    }

    pub fn run() {
        let args: Vec<String> = std::env::args().collect();
        if args.len() != 3 {
            eprintln!("usage: p9proxy <port-device> <mount-point>");
            std::process::exit(1);
        }

        let port_path = &args[1];
        let mount_point = &args[2];

        // Open the virtio-serial port device for reading and writing.
        let port = OpenOptions::new()
            .read(true)
            .write(true)
            .open(port_path)
            .unwrap_or_else(|e| {
                eprintln!("p9proxy: failed to open {port_path}: {e}");
                std::process::exit(1);
            });

        // Create a Unix socketpair. One end goes to the kernel (via mount),
        // the other stays with the proxy daemon for data copying.
        let (kernel_sock, proxy_sock) = UnixStream::pair().unwrap_or_else(|e| {
            eprintln!("p9proxy: socketpair failed: {e}");
            std::process::exit(1);
        });

        // The mount command inherits our fds via exec. Clear CLOEXEC so the
        // kernel-side socket fd survives the exec into `mount`.
        let kernel_fd = kernel_sock.as_raw_fd();
        clear_cloexec(kernel_fd);

        // Fork BEFORE mount: the child starts the proxy loop immediately so
        // 9P messages can flow when mount sends the initial Tversion. Without
        // this, mount blocks waiting for a response that nobody is proxying.
        let pid = unsafe { libc::fork() };
        match pid {
            -1 => {
                eprintln!("p9proxy: fork failed");
                std::process::exit(1);
            }
            0 => {
                // Child: close the kernel-side fd (only parent needs it for
                // mount). Run the proxy loop forever.
                drop(kernel_sock);
                proxy_loop(proxy_sock, port);
            }
            _ => {
                // Parent: close the proxy-side fd (child owns it).
                drop(proxy_sock);
                drop(port);

                // Call mount. The kernel will use kernel_fd to send/receive
                // 9P messages through the socketpair, which the child process
                // proxies to the virtio-serial port.
                let mount_opts = format!(
                    "version=9p2000.L,trans=fd,rfdno={kernel_fd},wfdno={kernel_fd},access=any,cache=none"
                );
                let status = Command::new("mount")
                    .args(["-t", "9p", "-o", &mount_opts, "p9proxy", mount_point])
                    .status()
                    .unwrap_or_else(|e| {
                        eprintln!("p9proxy: failed to exec mount: {e}");
                        unsafe { libc::kill(pid, libc::SIGTERM); }
                        std::process::exit(1);
                    });

                if !status.success() {
                    eprintln!("p9proxy: mount failed with {status}");
                    unsafe { libc::kill(pid, libc::SIGTERM); }
                    std::process::exit(1);
                }

                // Mount succeeded. The kernel holds the socket reference.
                // The child daemon keeps proxying data. Parent exits cleanly
                // so the caller (init.sh) can continue booting.
            }
        }
    }
}

#[cfg(unix)]
fn main() {
    proxy::run();
}

#[cfg(not(unix))]
fn main() {
    eprintln!("p9proxy only runs inside the guest VM (Linux)");
    std::process::exit(1);
}
