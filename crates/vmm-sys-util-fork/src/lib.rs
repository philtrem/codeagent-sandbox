// Copyright 2019 Intel Corporation. All Rights Reserved.
// SPDX-License-Identifier: BSD-3-Clause

//! Fork of vmm-sys-util 0.12.1 with macOS support for epoll (via kqueue),
//! eventfd (via pipe), and sock_ctrl_msg (SCM_RIGHTS).

#![deny(missing_docs, missing_debug_implementations)]
#![cfg_attr(docsrs, feature(doc_auto_cfg))]

#[cfg(unix)]
mod linux;
#[cfg(unix)]
pub use crate::linux::*;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use crate::unix::*;

pub mod errno;
pub mod fam;
pub mod metric;
pub mod rand;
pub mod syscall;
pub mod tempfile;
