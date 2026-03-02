// Copyright 2020 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: BSD-3-Clause

//! Safe wrappers over epoll (Linux) or kqueue (macOS) providing a unified API.

// ======================== Linux/Android implementation ========================
#[cfg(any(target_os = "linux", target_os = "android"))]
mod platform {
    use std::io;
    use std::ops::{Deref, Drop};
    use std::os::unix::io::{AsRawFd, RawFd};

    use bitflags::bitflags;
    use libc::{
        epoll_create1, epoll_ctl, epoll_event, epoll_wait, EPOLLERR, EPOLLET, EPOLLEXCLUSIVE,
        EPOLLHUP, EPOLLIN, EPOLLONESHOT, EPOLLOUT, EPOLLPRI, EPOLLRDHUP, EPOLLWAKEUP,
        EPOLL_CLOEXEC, EPOLL_CTL_ADD, EPOLL_CTL_DEL, EPOLL_CTL_MOD,
    };

    use crate::syscall::SyscallReturnCode;

    /// Wrapper over `EPOLL_CTL_*` operations that can be performed on a file descriptor.
    #[derive(Debug)]
    #[repr(i32)]
    pub enum ControlOperation {
        /// Add a file descriptor to the interest list.
        Add = EPOLL_CTL_ADD,
        /// Change the settings associated with a file descriptor that is
        /// already in the interest list.
        Modify = EPOLL_CTL_MOD,
        /// Remove a file descriptor from the interest list.
        Delete = EPOLL_CTL_DEL,
    }

    bitflags! {
        /// The type of events we can monitor a file descriptor for.
        pub struct EventSet: u32 {
            /// The associated file descriptor is available for read operations.
            const IN = EPOLLIN as u32;
            /// The associated file descriptor is available for write operations.
            const OUT = EPOLLOUT as u32;
            /// Error condition happened on the associated file descriptor.
            const ERROR = EPOLLERR as u32;
            /// This can be used to detect peer shutdown when using Edge Triggered monitoring.
            const READ_HANG_UP = EPOLLRDHUP as u32;
            /// Sets the Edge Triggered behavior for the associated file descriptor.
            /// The default behavior is Level Triggered.
            const EDGE_TRIGGERED = EPOLLET as u32;
            /// Hang up happened on the associated file descriptor. Note that `epoll_wait`
            /// will always wait for this event and it is not necessary to set it in events.
            const HANG_UP = EPOLLHUP as u32;
            /// There is an exceptional condition on that file descriptor. It is mostly used to
            /// set high priority for some data.
            const PRIORITY = EPOLLPRI as u32;
            /// The event is considered as being "processed" from the time when it is returned
            /// by a call to `epoll_wait` until the next call to `epoll_wait` on the same
            /// epoll file descriptor, the closure of that file descriptor, the removal of the
            /// event file descriptor via EPOLL_CTL_DEL, or the clearing of EPOLLWAKEUP
            /// for the event file descriptor via EPOLL_CTL_MOD.
            const WAKE_UP = EPOLLWAKEUP as u32;
            /// Sets the one-shot behavior for the associated file descriptor.
            const ONE_SHOT = EPOLLONESHOT as u32;
            /// Sets an exclusive wake up mode for the epoll file descriptor that is being
            /// attached to the associated file descriptor.
            /// When a wake up event occurs and multiple epoll file descriptors are attached to
            /// the same target file using this mode, one or more of the epoll file descriptors
            /// will receive an event with `epoll_wait`. The default here is for all those file
            /// descriptors to receive an event.
            const EXCLUSIVE = EPOLLEXCLUSIVE as u32;
        }
    }

    /// Wrapper over
    /// ['libc::epoll_event'](https://doc.rust-lang.org/1.8.0/libc/struct.epoll_event.html).
    #[repr(transparent)]
    #[derive(Clone, Copy)]
    pub struct EpollEvent(epoll_event);

    impl std::fmt::Debug for EpollEvent {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{{ events: {}, data: {} }}", self.events(), self.data())
        }
    }

    impl Deref for EpollEvent {
        type Target = epoll_event;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl Default for EpollEvent {
        fn default() -> Self {
            EpollEvent(epoll_event {
                events: 0u32,
                u64: 0u64,
            })
        }
    }

    impl EpollEvent {
        /// Create a new epoll_event instance.
        pub fn new(events: EventSet, data: u64) -> Self {
            EpollEvent(epoll_event {
                events: events.bits(),
                u64: data,
            })
        }

        /// Returns the `events` from the epoll_event.
        pub fn events(&self) -> u32 {
            self.events
        }

        /// Returns the `EventSet` corresponding to `epoll_event.events`.
        ///
        /// # Panics
        ///
        /// Panics if the epoll_event contains invalid events.
        pub fn event_set(&self) -> EventSet {
            EventSet::from_bits(self.events()).unwrap()
        }

        /// Returns the `data` from the epoll_event.
        pub fn data(&self) -> u64 {
            self.u64
        }

        /// Converts the data to a RawFd (lossy if data doesn't fit in i32).
        pub fn fd(&self) -> RawFd {
            self.u64 as i32
        }
    }

    /// Wrapper over epoll functionality.
    #[derive(Debug)]
    pub struct Epoll {
        epoll_fd: RawFd,
    }

    impl Epoll {
        /// Create a new epoll file descriptor.
        pub fn new() -> io::Result<Self> {
            let epoll_fd = SyscallReturnCode(
                // SAFETY: Safe because the return code is transformed by `into_result`.
                unsafe { epoll_create1(EPOLL_CLOEXEC) },
            )
            .into_result()?;
            Ok(Epoll { epoll_fd })
        }

        /// Wrapper for `libc::epoll_ctl`.
        pub fn ctl(
            &self,
            operation: ControlOperation,
            fd: RawFd,
            event: EpollEvent,
        ) -> io::Result<()> {
            SyscallReturnCode(
                // SAFETY: Safe because we give valid fd and epoll_event.
                unsafe {
                    epoll_ctl(
                        self.epoll_fd,
                        operation as i32,
                        fd,
                        &event as *const EpollEvent as *mut epoll_event,
                    )
                },
            )
            .into_empty_result()
        }

        /// Wrapper for `libc::epoll_wait`.
        pub fn wait(&self, timeout: i32, events: &mut [EpollEvent]) -> io::Result<usize> {
            let events_count = SyscallReturnCode(
                // SAFETY: Safe because we give a valid epoll fd and event array.
                unsafe {
                    epoll_wait(
                        self.epoll_fd,
                        events.as_mut_ptr() as *mut epoll_event,
                        events.len() as i32,
                        timeout,
                    )
                },
            )
            .into_result()? as usize;

            Ok(events_count)
        }
    }

    impl AsRawFd for Epoll {
        fn as_raw_fd(&self) -> RawFd {
            self.epoll_fd
        }
    }

    impl Drop for Epoll {
        fn drop(&mut self) {
            // SAFETY: Safe because this fd is opened with `epoll_create`.
            unsafe {
                libc::close(self.epoll_fd);
            }
        }
    }
}

// ======================== macOS implementation (kqueue) ========================
#[cfg(target_os = "macos")]
mod platform {
    use std::collections::HashMap;
    use std::io;
    use std::os::unix::io::{AsRawFd, RawFd};
    use std::sync::Mutex;

    use bitflags::bitflags;

    use crate::syscall::SyscallReturnCode;

    /// Wrapper over epoll control operations, implemented via kqueue on macOS.
    #[derive(Debug, Clone, Copy)]
    pub enum ControlOperation {
        /// Add a file descriptor to the interest list.
        Add,
        /// Change the settings associated with a file descriptor.
        Modify,
        /// Remove a file descriptor from the interest list.
        Delete,
    }

    bitflags! {
        /// The type of events we can monitor a file descriptor for.
        /// Values are chosen to match the Linux EPOLL* constants for consistency.
        pub struct EventSet: u32 {
            /// The associated file descriptor is available for read operations.
            const IN = 0x001;
            /// The associated file descriptor is available for write operations.
            const OUT = 0x004;
            /// Error condition happened on the associated file descriptor.
            const ERROR = 0x008;
            /// This can be used to detect peer shutdown when using Edge Triggered monitoring.
            const READ_HANG_UP = 0x2000;
            /// Sets the Edge Triggered behavior for the associated file descriptor.
            const EDGE_TRIGGERED = 0x8000_0000;
            /// Hang up happened on the associated file descriptor.
            const HANG_UP = 0x010;
            /// There is an exceptional condition on that file descriptor.
            const PRIORITY = 0x002;
            /// Wake up event processing marker.
            const WAKE_UP = 0x2000_0000;
            /// Sets the one-shot behavior for the associated file descriptor.
            const ONE_SHOT = 0x4000_0000;
            /// Sets an exclusive wake up mode.
            const EXCLUSIVE = 0x1000_0000;
        }
    }

    /// Event structure compatible with epoll's EpollEvent, backed by kqueue on macOS.
    #[derive(Clone, Copy)]
    pub struct EpollEvent {
        events: u32,
        data: u64,
    }

    impl std::fmt::Debug for EpollEvent {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{{ events: {}, data: {} }}", self.events, self.data)
        }
    }

    impl Default for EpollEvent {
        fn default() -> Self {
            EpollEvent {
                events: 0,
                data: 0,
            }
        }
    }

    impl EpollEvent {
        /// Create a new EpollEvent instance.
        pub fn new(events: EventSet, data: u64) -> Self {
            EpollEvent {
                events: events.bits(),
                data,
            }
        }

        /// Returns the raw events bitmask.
        pub fn events(&self) -> u32 {
            self.events
        }

        /// Returns the `EventSet` corresponding to the events.
        ///
        /// # Panics
        ///
        /// Panics if the events contain invalid bits.
        pub fn event_set(&self) -> EventSet {
            EventSet::from_bits(self.events).unwrap()
        }

        /// Returns the user data.
        pub fn data(&self) -> u64 {
            self.data
        }

        /// Converts the data to a RawFd (lossy if data doesn't fit in i32).
        pub fn fd(&self) -> RawFd {
            self.data as i32
        }
    }

    /// Tracks the registered state for a single file descriptor.
    #[derive(Debug, Clone)]
    struct Registration {
        events: EventSet,
        data: u64,
    }

    /// Epoll-compatible wrapper over kqueue.
    pub struct Epoll {
        kqueue_fd: RawFd,
        registrations: Mutex<HashMap<RawFd, Registration>>,
    }

    impl std::fmt::Debug for Epoll {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Epoll")
                .field("kqueue_fd", &self.kqueue_fd)
                .finish()
        }
    }

    impl Epoll {
        /// Create a new kqueue-backed Epoll instance.
        pub fn new() -> io::Result<Self> {
            let kq = SyscallReturnCode(
                // SAFETY: kqueue() is safe, we check the result.
                unsafe { libc::kqueue() },
            )
            .into_result()?;

            // Set close-on-exec
            // SAFETY: Valid fd from kqueue().
            unsafe {
                libc::fcntl(kq, libc::F_SETFD, libc::FD_CLOEXEC);
            }

            Ok(Epoll {
                kqueue_fd: kq,
                registrations: Mutex::new(HashMap::new()),
            })
        }

        /// Submit a kqueue changelist.
        fn submit_changes(&self, changes: &[libc::kevent]) -> io::Result<()> {
            if changes.is_empty() {
                return Ok(());
            }
            SyscallReturnCode(
                // SAFETY: Valid kqueue fd, valid changelist pointer, no eventlist.
                unsafe {
                    libc::kevent(
                        self.kqueue_fd,
                        changes.as_ptr(),
                        changes.len() as i32,
                        std::ptr::null_mut(),
                        0,
                        &libc::timespec {
                            tv_sec: 0,
                            tv_nsec: 0,
                        },
                    )
                },
            )
            .into_empty_result()
        }

        /// Build kqueue changelist entries for the given EventSet.
        fn build_changes(
            fd: RawFd,
            events: EventSet,
            data: u64,
            action: u16,
        ) -> Vec<libc::kevent> {
            let mut changes = Vec::new();
            let flags = action | libc::EV_RECEIPT as u16;
            let udata = data as usize as *mut libc::c_void;

            if events.contains(EventSet::IN) {
                changes.push(libc::kevent {
                    ident: fd as usize,
                    filter: libc::EVFILT_READ,
                    flags,
                    fflags: 0,
                    data: 0,
                    udata,
                });
            }
            if events.contains(EventSet::OUT) {
                changes.push(libc::kevent {
                    ident: fd as usize,
                    filter: libc::EVFILT_WRITE,
                    flags,
                    fflags: 0,
                    data: 0,
                    udata,
                });
            }
            changes
        }

        /// Add, modify, or delete a file descriptor in the kqueue interest list.
        pub fn ctl(
            &self,
            operation: ControlOperation,
            fd: RawFd,
            event: EpollEvent,
        ) -> io::Result<()> {
            let mut regs = self.registrations.lock().unwrap();
            let event_set = EventSet::from_bits_truncate(event.events());

            match operation {
                ControlOperation::Add => {
                    if regs.contains_key(&fd) {
                        return Err(io::Error::from_raw_os_error(libc::EEXIST));
                    }
                    let changes =
                        Self::build_changes(fd, event_set, event.data(), libc::EV_ADD as u16);
                    self.submit_changes(&changes)?;
                    regs.insert(
                        fd,
                        Registration {
                            events: event_set,
                            data: event.data(),
                        },
                    );
                    Ok(())
                }
                ControlOperation::Modify => {
                    let old = regs.get(&fd).ok_or_else(|| {
                        io::Error::from_raw_os_error(libc::ENOENT)
                    })?;
                    let old_events = old.events;

                    // Delete filters that are no longer wanted
                    let removed = old_events & !event_set;
                    if !removed.is_empty() {
                        let del_changes =
                            Self::build_changes(fd, removed, 0, libc::EV_DELETE as u16);
                        // Ignore errors from deleting non-existent filters
                        let _ = self.submit_changes(&del_changes);
                    }

                    // Add/update filters that are now wanted
                    if !event_set.is_empty() {
                        let add_changes = Self::build_changes(
                            fd,
                            event_set,
                            event.data(),
                            libc::EV_ADD as u16,
                        );
                        self.submit_changes(&add_changes)?;
                    }

                    regs.insert(
                        fd,
                        Registration {
                            events: event_set,
                            data: event.data(),
                        },
                    );
                    Ok(())
                }
                ControlOperation::Delete => {
                    let old = regs.remove(&fd).ok_or_else(|| {
                        io::Error::from_raw_os_error(libc::ENOENT)
                    })?;

                    let changes =
                        Self::build_changes(fd, old.events, 0, libc::EV_DELETE as u16);
                    // Best-effort delete; the fd might already be closed
                    let _ = self.submit_changes(&changes);
                    Ok(())
                }
            }
        }

        /// Wait for events on the kqueue, translating to epoll-compatible EpollEvents.
        pub fn wait(&self, timeout: i32, events: &mut [EpollEvent]) -> io::Result<usize> {
            let max_events = events.len();
            // Allocate double capacity to handle potential per-filter events that need merging
            let raw_capacity = max_events * 2;
            let mut raw_events: Vec<libc::kevent> =
                vec![unsafe { std::mem::zeroed() }; raw_capacity];

            let ts_storage;
            let ts_ptr = if timeout < 0 {
                std::ptr::null()
            } else {
                ts_storage = libc::timespec {
                    tv_sec: (timeout / 1000) as libc::time_t,
                    tv_nsec: ((timeout % 1000) as i64) * 1_000_000,
                };
                &ts_storage as *const libc::timespec
            };

            let count = SyscallReturnCode(
                // SAFETY: Valid kqueue fd, valid event array, valid timespec pointer.
                unsafe {
                    libc::kevent(
                        self.kqueue_fd,
                        std::ptr::null(),
                        0,
                        raw_events.as_mut_ptr(),
                        raw_capacity as i32,
                        ts_ptr,
                    )
                },
            )
            .into_result()? as usize;

            // Merge kqueue events by fd (ident), translating filters to EventSet bits.
            // Use a small vec to track which output slots map to which idents.
            let mut result_count: usize = 0;
            let mut ident_to_slot: Vec<(usize, usize)> = Vec::new(); // (ident, slot)

            for raw in &raw_events[..count] {
                let mut event_bits: u32 = 0;

                match raw.filter {
                    libc::EVFILT_READ => event_bits |= EventSet::IN.bits(),
                    libc::EVFILT_WRITE => event_bits |= EventSet::OUT.bits(),
                    _ => {}
                }

                if raw.flags & libc::EV_EOF != 0 {
                    event_bits |= EventSet::HANG_UP.bits();
                }
                if raw.flags & libc::EV_ERROR != 0 {
                    event_bits |= EventSet::ERROR.bits();
                }

                let ident = raw.ident;
                let data = raw.udata as u64;

                // Try to merge with existing event for same ident
                if let Some(&(_, slot)) = ident_to_slot.iter().find(|&&(id, _)| id == ident) {
                    let existing = &mut events[slot];
                    *existing = EpollEvent::new(
                        EventSet::from_bits_truncate(existing.events() | event_bits),
                        data,
                    );
                } else if result_count < max_events {
                    events[result_count] = EpollEvent::new(
                        EventSet::from_bits_truncate(event_bits),
                        data,
                    );
                    ident_to_slot.push((ident, result_count));
                    result_count += 1;
                }
            }

            Ok(result_count)
        }
    }

    impl AsRawFd for Epoll {
        fn as_raw_fd(&self) -> RawFd {
            self.kqueue_fd
        }
    }

    impl Drop for Epoll {
        fn drop(&mut self) {
            // SAFETY: Safe because this fd is opened with `kqueue`.
            unsafe {
                libc::close(self.kqueue_fd);
            }
        }
    }
}

pub use platform::*;

#[cfg(test)]
mod tests {
    use super::*;

    use crate::eventfd::EventFd;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    use std::os::unix::io::AsRawFd;

    #[test]
    fn test_event_ops() {
        let mut event = EpollEvent::default();
        assert_eq!(event.events(), 0);
        assert_eq!(event.data(), 0);

        event = EpollEvent::new(EventSet::IN, 2);
        assert_eq!(event.event_set(), EventSet::IN);
        assert_eq!(event.data(), 2);
        assert_eq!(event.fd(), 2);
    }

    #[test]
    fn test_events_debug() {
        let events = EpollEvent::new(EventSet::IN, 42);
        let debug = format!("{:?}", events);
        assert!(debug.contains("42"));
    }

    #[test]
    fn test_epoll_add_wait_delete() {
        let epoll = Epoll::new().unwrap();
        let event_fd = EventFd::new(super::super::eventfd::EFD_NONBLOCK).unwrap();
        event_fd.write(1).unwrap();

        let fd = event_fd.as_raw_fd();
        epoll
            .ctl(
                ControlOperation::Add,
                fd,
                EpollEvent::new(EventSet::IN, fd as u64),
            )
            .unwrap();

        let mut ready_events = vec![EpollEvent::default(); 10];
        let ev_count = epoll.wait(100, &mut ready_events[..]).unwrap();
        assert!(ev_count >= 1);

        epoll
            .ctl(
                ControlOperation::Delete,
                fd,
                EpollEvent::default(),
            )
            .unwrap();
    }

    #[test]
    fn test_epoll_add_duplicate_fails() {
        let epoll = Epoll::new().unwrap();
        let event_fd = EventFd::new(super::super::eventfd::EFD_NONBLOCK).unwrap();
        let fd = event_fd.as_raw_fd();

        epoll
            .ctl(
                ControlOperation::Add,
                fd,
                EpollEvent::new(EventSet::IN, fd as u64),
            )
            .unwrap();

        assert!(epoll
            .ctl(
                ControlOperation::Add,
                fd,
                EpollEvent::new(EventSet::IN, fd as u64),
            )
            .is_err());
    }

    #[test]
    fn test_epoll_delete_nonexistent_fails() {
        let epoll = Epoll::new().unwrap();
        let event_fd = EventFd::new(super::super::eventfd::EFD_NONBLOCK).unwrap();
        let fd = event_fd.as_raw_fd();

        assert!(epoll
            .ctl(
                ControlOperation::Delete,
                fd,
                EpollEvent::default(),
            )
            .is_err());
    }
}
