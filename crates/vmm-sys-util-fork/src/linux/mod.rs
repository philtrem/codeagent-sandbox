// Copyright 2022 rust-vmm Authors or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: BSD-3-Clause

// Modules available on all Unix platforms (with platform-specific implementations):
pub mod epoll;
pub mod eventfd;
pub mod sock_ctrl_msg;

// Modules that are Linux/Android-only:
#[cfg(any(target_os = "linux", target_os = "android"))]
#[macro_use]
pub mod ioctl;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod aio;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod fallocate;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod poll;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod seek_hole;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod signal;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod timerfd;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod write_zeroes;
