// Copyright 2024 Red Hat, Inc. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/*!
 * Module for migrating our internal FS state (i.e. serializing and deserializing it), with the
 * following submodules:
 * - serialized: Serialized data structures
 * - preserialization: Structures and functionality for preparing for migration (serialization),
 *   i.e. define and construct the precursors to the eventually serialized information that are
 *   stored alongside the associated inodes and handles they describe
 * - serialization: Functionality for serializing
 * - deserialization: Functionality for deserializing
 */

mod deserialization;
pub(super) mod preserialization;
mod serialization;
mod serialized;

use crate::filesystem::SerializableFileSystem;
use crate::passthrough::{MigrationMode, PassthroughFs};
use preserialization::{file_handles, find_paths};
use std::convert::{TryFrom, TryInto};
use std::fs::File;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[cfg(target_os = "linux")]
use preserialization::proc_paths::{self, ConfirmPaths, ImplicitPathCheck};

/// Adds serialization (migration) capabilities to `PassthroughFs`
impl SerializableFileSystem for PassthroughFs {
    fn prepare_serialization(&self, cancel: Arc<AtomicBool>) {
        self.inodes.clear_migration_info();

        self.track_migration_info.store(true, Ordering::Relaxed);

        match self.cfg.migration_mode {
            MigrationMode::FindPaths => {
                #[cfg(target_os = "linux")]
                {
                    // Try proc_paths first, fall back to find_paths if needed
                    if proc_paths::Constructor::new(self, Arc::clone(&cancel)).execute() {
                        warn!("Falling back to iterating through the shared directory to reconstruct paths for migration");
                        find_paths::Constructor::new(self, Arc::clone(&cancel)).execute();
                    }
                }
                #[cfg(not(target_os = "linux"))]
                {
                    // On macOS, proc_paths is not available; use find_paths directly
                    find_paths::Constructor::new(self, Arc::clone(&cancel)).execute();
                }
            }

            MigrationMode::FileHandles => {
                file_handles::Constructor::new(self, Arc::clone(&cancel)).execute();
            }
        }

        // Re-check paths after preserialization to catch TOCTTOU races
        #[cfg(target_os = "linux")]
        {
            let checker = ImplicitPathCheck::new(self, cancel);
            checker.check_paths();
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = cancel; // suppress unused warning
        }
    }

    fn serialize(&self, mut state_pipe: File) -> io::Result<()> {
        self.track_migration_info.store(false, Ordering::Relaxed);

        if self.cfg.migration_confirm_paths {
            #[cfg(target_os = "linux")]
            {
                let checker = ConfirmPaths::new(self);
                if let Err(err) = checker.confirm_paths() {
                    self.inodes.clear_migration_info();
                    return Err(err);
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                warn!("migration_confirm_paths is not supported on this platform");
            }
        }

        let state = serialized::PassthroughFs::V2(self.into());
        self.inodes.clear_migration_info();
        let serialized: Vec<u8> = state.try_into()?;
        state_pipe.write_all(&serialized)?;
        Ok(())
    }

    fn deserialize_and_apply(&self, mut state_pipe: File) -> io::Result<()> {
        let mut serialized: Vec<u8> = Vec::new();
        state_pipe.read_to_end(&mut serialized)?;
        match serialized::PassthroughFs::try_from(serialized)? {
            serialized::PassthroughFs::V1(state) => state.apply(self)?,
            serialized::PassthroughFs::V2(state) => state.apply(self)?,
        };
        Ok(())
    }
}
