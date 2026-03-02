// Copyright 2019 Intel Corporation. All Rights Reserved.
//
// Copyright 2017 The Chromium OS Authors. All rights reserved.
//
// SPDX-License-Identifier: BSD-3-Clause

//! Structure and wrapper functions for working with eventfd (Linux) or pipe-based
//! signaling (macOS), providing a unified API.

// ======================== Linux/Android implementation ========================
#[cfg(any(target_os = "linux", target_os = "android"))]
mod platform {
    use std::fs::File;
    use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
    use std::{io, mem, result};

    use libc::{c_void, dup, eventfd, read, write};

    pub use libc::{EFD_CLOEXEC, EFD_NONBLOCK, EFD_SEMAPHORE};

    /// A safe wrapper around Linux eventfd.
    #[derive(Debug)]
    pub struct EventFd {
        eventfd: File,
    }

    impl EventFd {
        /// Create a new EventFd with an initial value.
        pub fn new(flag: i32) -> result::Result<EventFd, io::Error> {
            // SAFETY: Safe because eventfd merely allocates an eventfd.
            let ret = unsafe { eventfd(0, flag) };
            if ret < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(EventFd {
                    // SAFETY: Safe because we checked ret for success.
                    eventfd: unsafe { File::from_raw_fd(ret) },
                })
            }
        }

        /// Add a value to the eventfd's counter.
        pub fn write(&self, v: u64) -> result::Result<(), io::Error> {
            // SAFETY: Safe because we own this fd and pass correct size.
            let ret = unsafe {
                write(
                    self.as_raw_fd(),
                    &v as *const u64 as *const c_void,
                    mem::size_of::<u64>(),
                )
            };
            if ret <= 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        /// Read a value from the eventfd.
        pub fn read(&self) -> result::Result<u64, io::Error> {
            let mut buf: u64 = 0;
            // SAFETY: Safe because we own this fd and pass correct size.
            let ret = unsafe {
                read(
                    self.as_raw_fd(),
                    &mut buf as *mut u64 as *mut c_void,
                    mem::size_of::<u64>(),
                )
            };
            if ret < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(buf)
            }
        }

        /// Clone this EventFd, sharing the same underlying counter.
        pub fn try_clone(&self) -> result::Result<EventFd, io::Error> {
            // SAFETY: Safe because we own this fd and check the result.
            let ret = unsafe { dup(self.as_raw_fd()) };
            if ret < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(EventFd {
                    // SAFETY: Safe because we checked ret for success.
                    eventfd: unsafe { File::from_raw_fd(ret) },
                })
            }
        }
    }

    impl AsRawFd for EventFd {
        fn as_raw_fd(&self) -> RawFd {
            self.eventfd.as_raw_fd()
        }
    }

    impl FromRawFd for EventFd {
        unsafe fn from_raw_fd(fd: RawFd) -> Self {
            EventFd {
                eventfd: File::from_raw_fd(fd),
            }
        }
    }
}

// ======================== macOS implementation (pipe-based) ========================
#[cfg(target_os = "macos")]
mod platform {
    use std::fs::File;
    use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
    use std::{io, mem, result};

    use libc::{c_void, read, write};

    /// Equivalent of EFD_NONBLOCK for pipe-based EventFd.
    pub const EFD_NONBLOCK: i32 = libc::O_NONBLOCK;
    /// Equivalent of EFD_CLOEXEC for pipe-based EventFd.
    pub const EFD_CLOEXEC: i32 = 0x100_0000; // FD_CLOEXEC is handled via fcntl
    /// Semaphore mode (not supported with pipe-based implementation).
    pub const EFD_SEMAPHORE: i32 = 0;

    /// A pipe-based signaling mechanism compatible with the Linux EventFd API.
    ///
    /// On macOS, there is no eventfd syscall. This implementation uses a pipe pair:
    /// writes go to the write end, reads come from the read end, and the read end fd
    /// is used for kqueue/poll monitoring.
    #[derive(Debug)]
    pub struct EventFd {
        read_file: File,
        write_file: File,
    }

    impl EventFd {
        /// Create a new EventFd (pipe-based on macOS).
        ///
        /// The `flag` parameter is interpreted for O_NONBLOCK and O_CLOEXEC bits.
        pub fn new(flag: i32) -> result::Result<EventFd, io::Error> {
            let mut pipe_fds = [0i32; 2];
            // SAFETY: Safe because pipe_fds is a valid pointer to two i32s.
            let ret = unsafe { libc::pipe(pipe_fds.as_mut_ptr()) };
            if ret < 0 {
                return Err(io::Error::last_os_error());
            }

            let read_fd = pipe_fds[0];
            let write_fd = pipe_fds[1];

            // Apply O_NONBLOCK to both ends if requested
            if flag & libc::O_NONBLOCK != 0 {
                for &fd in &[read_fd, write_fd] {
                    // SAFETY: Safe because fd is valid from pipe().
                    let fl = unsafe { libc::fcntl(fd, libc::F_GETFL) };
                    if fl < 0 {
                        unsafe {
                            libc::close(read_fd);
                            libc::close(write_fd);
                        }
                        return Err(io::Error::last_os_error());
                    }
                    // SAFETY: Safe with valid fd.
                    let ret = unsafe { libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK) };
                    if ret < 0 {
                        unsafe {
                            libc::close(read_fd);
                            libc::close(write_fd);
                        }
                        return Err(io::Error::last_os_error());
                    }
                }
            }

            // Apply FD_CLOEXEC to both ends
            for &fd in &[read_fd, write_fd] {
                // SAFETY: Safe because fd is valid from pipe().
                unsafe {
                    libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                }
            }

            Ok(EventFd {
                // SAFETY: Safe because we verified the pipe fds are valid.
                read_file: unsafe { File::from_raw_fd(read_fd) },
                write_file: unsafe { File::from_raw_fd(write_fd) },
            })
        }

        /// Write a u64 value to the pipe.
        pub fn write(&self, v: u64) -> result::Result<(), io::Error> {
            // SAFETY: Safe because we own the write fd and pass the correct size.
            let ret = unsafe {
                write(
                    self.write_file.as_raw_fd(),
                    &v as *const u64 as *const c_void,
                    mem::size_of::<u64>(),
                )
            };
            if ret <= 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        /// Read a u64 value from the pipe.
        pub fn read(&self) -> result::Result<u64, io::Error> {
            let mut buf: u64 = 0;
            // SAFETY: Safe because we own the read fd and pass the correct size.
            let ret = unsafe {
                read(
                    self.read_file.as_raw_fd(),
                    &mut buf as *mut u64 as *mut c_void,
                    mem::size_of::<u64>(),
                )
            };
            if ret < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(buf)
            }
        }

        /// Clone this EventFd by duplicating both pipe endpoints.
        pub fn try_clone(&self) -> result::Result<EventFd, io::Error> {
            // SAFETY: Safe because we own these fds and check results.
            let new_read = unsafe { libc::dup(self.read_file.as_raw_fd()) };
            if new_read < 0 {
                return Err(io::Error::last_os_error());
            }
            let new_write = unsafe { libc::dup(self.write_file.as_raw_fd()) };
            if new_write < 0 {
                unsafe { libc::close(new_read) };
                return Err(io::Error::last_os_error());
            }
            Ok(EventFd {
                // SAFETY: Safe because dup returned valid fds.
                read_file: unsafe { File::from_raw_fd(new_read) },
                write_file: unsafe { File::from_raw_fd(new_write) },
            })
        }
    }

    impl AsRawFd for EventFd {
        fn as_raw_fd(&self) -> RawFd {
            self.read_file.as_raw_fd()
        }
    }

    impl FromRawFd for EventFd {
        /// Create an EventFd from a raw file descriptor.
        ///
        /// On macOS, this creates a new pipe and uses the provided fd as the read end.
        /// The write end will be a new pipe endpoint that is NOT connected to the
        /// provided fd. This is a best-effort compatibility shim; prefer `new()` instead.
        unsafe fn from_raw_fd(fd: RawFd) -> Self {
            let mut pipe_fds = [0i32; 2];
            let ret = libc::pipe(pipe_fds.as_mut_ptr());
            if ret < 0 {
                panic!("EventFd::from_raw_fd: pipe() failed");
            }
            // Close the read end of the new pipe; we use the provided fd instead
            libc::close(pipe_fds[0]);
            EventFd {
                read_file: File::from_raw_fd(fd),
                write_file: File::from_raw_fd(pipe_fds[1]),
            }
        }
    }
}

pub use platform::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        EventFd::new(EFD_NONBLOCK).unwrap();
    }

    #[test]
    fn test_read_write() {
        let evt = EventFd::new(EFD_NONBLOCK).unwrap();
        evt.write(55).unwrap();
        assert_eq!(evt.read().unwrap(), 55);
    }

    #[test]
    fn test_read_nothing() {
        let evt = EventFd::new(EFD_NONBLOCK).unwrap();
        let r = evt.read();
        match r {
            Err(ref inner) if inner.kind() == std::io::ErrorKind::WouldBlock => (),
            _ => panic!("Expected WouldBlock error"),
        }
    }

    #[test]
    fn test_clone() {
        let evt = EventFd::new(EFD_NONBLOCK).unwrap();
        let evt_clone = evt.try_clone().unwrap();
        evt.write(923).unwrap();
        assert_eq!(evt_clone.read().unwrap(), 923);
    }
}
