// Copyright 2022 Red Hat, Inc. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fs::{File, OpenOptions};
use std::io::{Error, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::{fs, io, process};

fn try_lock_file(file: &File) -> Result<(), Error> {
    let file_fd = file.as_raw_fd();
    let ret = unsafe { libc::flock(file_fd, libc::LOCK_EX | libc::LOCK_NB) };
    if ret == -1 {
        return Err(Error::last_os_error());
    }
    Ok(())
}

pub fn write_pid_file(pid_file_name: &Path) -> Result<File, std::io::Error> {
    let mut pid_file = loop {
        let file = OpenOptions::new()
            .mode(libc::S_IRUSR | libc::S_IWUSR)
            .custom_flags(libc::O_CLOEXEC)
            .write(true)
            .create(true)
            .open(pid_file_name)?;

        try_lock_file(&file)?;

        let locked = file.metadata()?.ino();
        let current = match fs::metadata(pid_file_name) {
            Ok(stat) => stat.ino(),
            _ => continue,
        };

        if locked == current {
            break file;
        }
    };

    let pid = format!("{}\n", process::id());
    pid_file.write_all(pid.as_bytes())?;

    Ok(pid_file)
}

// Linux-only: pidfd_open, sfork, wait_for_child (use pidfd, prctl, capng)
#[cfg(target_os = "linux")]
mod linux_only {
    use super::*;
    use std::os::unix::io::FromRawFd;

    unsafe fn pidfd_open(pid: libc::pid_t, flags: libc::c_uint) -> libc::c_int {
        libc::syscall(libc::SYS_pidfd_open, pid, flags) as libc::c_int
    }

    pub fn sfork() -> io::Result<i32> {
        let cur_pid = unsafe { libc::getpid() };

        let parent_pidfd = unsafe { pidfd_open(cur_pid, 0) };
        if parent_pidfd == -1 {
            return Err(Error::last_os_error());
        }

        #[allow(dead_code)]
        struct PidFd(File);
        let _pidfd = unsafe { PidFd(File::from_raw_fd(parent_pidfd)) };

        let child_pid = unsafe { libc::fork() };
        if child_pid == -1 {
            return Err(Error::last_os_error());
        }

        if child_pid == 0 {
            let ret = unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) };
            assert_eq!(ret, 0);

            let mut pollfds = libc::pollfd {
                fd: parent_pidfd,
                events: libc::POLLIN,
                revents: 0,
            };
            let num_fds = unsafe { libc::poll(&mut pollfds, 1, 0) };
            if num_fds == -1 {
                return Err(io::Error::last_os_error());
            }
            if num_fds != 0 {
                return Err(super::other_io_error("Parent process died unexpectedly"));
            }
        }
        Ok(child_pid)
    }

    pub fn wait_for_child(pid: i32) -> ! {
        capng::clear(capng::Set::BOTH);
        if let Err(e) = capng::apply(capng::Set::BOTH) {
            error!("warning: can't apply the parent capabilities: {e}");
        }

        let mut status = 0;
        if unsafe { libc::waitpid(pid, &mut status, 0) } != pid {
            error!("Error during waitpid()");
            process::exit(1);
        }

        let exit_code = if libc::WIFEXITED(status) {
            libc::WEXITSTATUS(status)
        } else if libc::WIFSIGNALED(status) {
            let signal = libc::WTERMSIG(status);
            error!("Child process terminated by signal {signal}");
            -signal
        } else {
            error!("Unexpected waitpid status: {status:#X}");
            libc::EXIT_FAILURE
        };

        process::exit(exit_code);
    }

    pub fn add_cap_to_eff(cap_name: &str) -> capng::Result<()> {
        use capng::{Action, CUpdate, Set, Type};
        let cap = capng::name_to_capability(cap_name)?;
        capng::get_caps_process()?;

        let req = vec![CUpdate {
            action: Action::ADD,
            cap_type: Type::EFFECTIVE,
            capability: cap,
        }];

        capng::update(req)?;
        capng::apply(Set::CAPS)?;

        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub use linux_only::{add_cap_to_eff, sfork, wait_for_child};

pub fn other_io_error<E: Into<Box<dyn std::error::Error + Send + Sync>>>(err: E) -> io::Error {
    #[allow(unknown_lints)]
    #[allow(clippy::io_other_error)]
    io::Error::new(io::ErrorKind::Other, err)
}

pub trait ErrorContext {
    fn context<C: std::fmt::Display>(self, context: C) -> Self;
}

impl ErrorContext for io::Error {
    fn context<C: std::fmt::Display>(self, context: C) -> Self {
        io::Error::new(self.kind(), format!("{context}: {self}"))
    }
}

pub trait ResultErrorContext {
    fn err_context<C: std::fmt::Display, F: FnOnce() -> C>(self, context: F) -> Self;
}

impl<V, E: ErrorContext> ResultErrorContext for Result<V, E> {
    fn err_context<C: std::fmt::Display, F: FnOnce() -> C>(self, context: F) -> Self {
        self.map_err(|err| err.context(context()))
    }
}
