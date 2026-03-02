// SPDX-License-Identifier: Apache-2.0 AND BSD-3-Clause

//! Platform compatibility layer for macOS support.
//!
//! Provides cross-platform abstractions over Linux-specific APIs used by
//! virtiofsd's PassthroughFs. On Linux, calls the native syscalls directly.
//! On macOS, uses equivalent BSD/POSIX APIs where available.

pub mod credentials;
pub mod fd_ops;
pub mod io_ops;
pub mod os_facts;
pub mod rename_ops;
pub mod stat_ops;
pub mod types;
