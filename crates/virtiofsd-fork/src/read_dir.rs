// Copyright 2020 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::compat::types::off64_t;
use crate::filesystem::{DirEntry, DirectoryIterator};

use std::ffi::CStr;
use std::io;
use std::mem::size_of;
use std::ops::{Deref, DerefMut};
use std::os::unix::io::AsRawFd;

use vm_memory::ByteValued;

// ======================== Linux implementation ========================
#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    #[repr(C, packed)]
    #[derive(Default, Clone, Copy)]
    pub(super) struct LinuxDirent64 {
        pub d_ino: libc::ino64_t,
        pub d_off: libc::off64_t,
        pub d_reclen: libc::c_ushort,
        pub d_ty: libc::c_uchar,
    }
    unsafe impl ByteValued for LinuxDirent64 {}

    pub fn lseek64<D: AsRawFd>(dir: &D, offset: i64) -> io::Result<()> {
        let res = unsafe { libc::lseek64(dir.as_raw_fd(), offset, libc::SEEK_SET) };
        if res < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub unsafe fn getdents<D: AsRawFd>(dir: &D, buf: &mut [u8]) -> io::Result<usize> {
        let res = unsafe {
            libc::syscall(
                libc::SYS_getdents64,
                dir.as_raw_fd(),
                buf.as_mut_ptr() as *mut LinuxDirent64,
                buf.len() as libc::c_int,
            )
        };
        if res < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(res as usize)
        }
    }

    pub fn parse_entry(rem: &[u8]) -> Option<(DirEntry<'_>, usize)> {
        if rem.is_empty() {
            return None;
        }

        debug_assert!(
            rem.len() >= size_of::<LinuxDirent64>(),
            "not enough space left in `rem`"
        );

        let (front, back) = rem.split_at(size_of::<LinuxDirent64>());

        let dirent64 =
            LinuxDirent64::from_slice(front).expect("unable to get LinuxDirent64 from slice");

        let namelen = dirent64.d_reclen as usize - size_of::<LinuxDirent64>();
        debug_assert!(namelen <= back.len(), "back is smaller than `namelen`");

        let name = super::strip_padding(&back[..namelen]);
        let entry = DirEntry {
            ino: dirent64.d_ino,
            offset: dirent64.d_off as u64,
            type_: dirent64.d_ty as u32,
            name,
        };

        Some((entry, dirent64.d_reclen as usize))
    }
}

// ======================== macOS implementation ========================
#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    pub fn lseek64<D: AsRawFd>(dir: &D, offset: i64) -> io::Result<()> {
        // macOS lseek is already 64-bit
        let res = unsafe { libc::lseek(dir.as_raw_fd(), offset as libc::off_t, libc::SEEK_SET) };
        if res < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub unsafe fn getdents<D: AsRawFd>(dir: &D, buf: &mut [u8]) -> io::Result<usize> {
        // macOS uses __getdirentries64 (or getdirentries with 64-bit support).
        // The buffer receives struct dirent entries.
        let mut basep: libc::c_long = 0;
        let res = unsafe {
            libc::getdirentries(
                dir.as_raw_fd(),
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len() as libc::c_int,
                &mut basep,
            )
        };
        if res < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(res as usize)
        }
    }

    pub fn parse_entry(rem: &[u8]) -> Option<(DirEntry<'_>, usize)> {
        if rem.is_empty() {
            return None;
        }

        // macOS struct dirent layout: d_ino (u64), d_seekoff (u64),
        // d_reclen (u16), d_namlen (u16), d_type (u8), d_name[...]
        // Minimum size: 8 + 8 + 2 + 2 + 1 = 21 bytes
        if rem.len() < 21 {
            return None;
        }

        // Read fields manually to handle packed struct correctly
        let d_ino = u64::from_ne_bytes(rem[0..8].try_into().unwrap());
        let d_seekoff = u64::from_ne_bytes(rem[8..16].try_into().unwrap());
        let d_reclen = u16::from_ne_bytes(rem[16..18].try_into().unwrap()) as usize;
        let _d_namlen = u16::from_ne_bytes(rem[18..20].try_into().unwrap());
        let d_type = rem[20];

        if d_reclen == 0 || d_reclen > rem.len() {
            return None;
        }

        let name_bytes = &rem[21..d_reclen];
        let name = super::strip_padding(name_bytes);

        let entry = DirEntry {
            ino: d_ino,
            offset: d_seekoff,
            type_: d_type as u32,
            name,
        };

        Some((entry, d_reclen))
    }
}

#[derive(Default)]
pub struct ReadDir<P> {
    buf: P,
    current: usize,
    end: usize,
}

impl<P: DerefMut<Target = [u8]>> ReadDir<P> {
    pub fn new<D: AsRawFd>(dir: &D, offset: off64_t, buf: P) -> io::Result<Self> {
        platform::lseek64(dir, offset)?;
        // Safe because we used lseek() to get to the correct position
        unsafe { Self::new_no_seek(dir, buf) }
    }

    /// Continue reading from the current position in the directory without seeking.
    ///
    /// # Safety
    /// Caller must ensure the current position is valid, for example, by exclusively using this
    /// function on a given FD, potentially repeatedly.
    pub unsafe fn new_no_seek<D: AsRawFd>(dir: &D, mut buf: P) -> io::Result<Self> {
        let end = unsafe { platform::getdents(dir, &mut buf)? };
        Ok(ReadDir {
            buf,
            current: 0,
            end,
        })
    }
}

impl<P> ReadDir<P> {
    /// Returns the number of bytes from the internal buffer that have not yet been consumed.
    pub fn remaining(&self) -> usize {
        self.end.saturating_sub(self.current)
    }
}

impl<P: Deref<Target = [u8]>> DirectoryIterator for ReadDir<P> {
    fn next(&mut self) -> Option<DirEntry<'_>> {
        let rem = &self.buf[self.current..self.end];
        let (entry, advance) = platform::parse_entry(rem)?;
        self.current += advance;
        Some(entry)
    }
}

// Like `CStr::from_bytes_with_nul` but strips any bytes after the first '\0'-byte. Panics if `b`
// doesn't contain any '\0' bytes.
fn strip_padding(b: &[u8]) -> &CStr {
    // It would be nice if we could use memchr here but that's locked behind an unstable gate.
    let pos = b
        .iter()
        .position(|&c| c == 0)
        .expect("`b` doesn't contain any nul bytes");

    // Safe because we are creating this string with the first nul-byte we found so we can
    // guarantee that it is nul-terminated and doesn't contain any interior nuls.
    unsafe { CStr::from_bytes_with_nul_unchecked(&b[..=pos]) }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn padded_cstrings() {
        assert_eq!(strip_padding(b".\0\0\0\0\0\0\0").to_bytes(), b".");
        assert_eq!(strip_padding(b"..\0\0\0\0\0\0").to_bytes(), b"..");
        assert_eq!(
            strip_padding(b"normal cstring\0").to_bytes(),
            b"normal cstring"
        );
        assert_eq!(strip_padding(b"\0\0\0\0").to_bytes(), b"");
        assert_eq!(
            strip_padding(b"interior\0nul bytes\0\0\0").to_bytes(),
            b"interior"
        );
    }

    #[test]
    #[should_panic(expected = "`b` doesn't contain any nul bytes")]
    fn no_nul_byte() {
        strip_padding(b"no nul bytes in string");
    }
}
