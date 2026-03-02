use std::ffi::CStr;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use virtiofsd::filesystem::{
    Context, Entry, Extensions, FileSystem, FsOptions, GetxattrReply, ListxattrReply, OpenOptions,
    SetattrValid, SerializableFileSystem, SetxattrFlags, ZeroCopyReader, ZeroCopyWriter,
};
use virtiofsd::fuse::{Attr, SetattrIn};
use virtiofsd::passthrough::PassthroughFs;

use codeagent_control::InFlightTracker;
use codeagent_interceptor::write_interceptor::WriteInterceptor;

use crate::inode_map::InodePathMap;

/// Drop guard that calls `InFlightTracker::end_operation()` on drop.
///
/// Ensures the in-flight count is always decremented, even on early return
/// or error paths.
struct InFlightGuard<'a> {
    tracker: &'a InFlightTracker,
}

impl<'a> InFlightGuard<'a> {
    fn new(tracker: &'a InFlightTracker) -> Self {
        tracker.begin_operation();
        Self { tracker }
    }
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.tracker.end_operation();
    }
}

/// Wraps `PassthroughFs` to intercept mutating filesystem operations.
///
/// Implements virtiofsd's `FileSystem` trait by delegating all operations to
/// the inner `PassthroughFs`, while calling `WriteInterceptor` pre/post hooks
/// on mutating operations and tracking in-flight operations via `InFlightTracker`.
///
/// Read-only methods are delegated directly (except `lookup` which also updates
/// the inode map). Mutating methods follow this pattern:
///
/// 1. `in_flight.begin_operation()` (via InFlightGuard)
/// 2. Call `WriteInterceptor` pre-hook (if applicable)
/// 3. If pre-hook returns error -> convert to `io::Error(EACCES)` and return
/// 4. Delegate to `inner.method()`
/// 5. Call `WriteInterceptor` post-hook (if applicable)
/// 6. Update `InodePathMap`
/// 7. `in_flight.end_operation()` (via InFlightGuard drop)
pub struct InterceptedFs {
    inner: PassthroughFs,
    interceptor: Arc<dyn WriteInterceptor>,
    in_flight: InFlightTracker,
    inode_map: InodePathMap,
}

impl InterceptedFs {
    pub fn new(
        inner: PassthroughFs,
        interceptor: Arc<dyn WriteInterceptor>,
        in_flight: InFlightTracker,
        root_dir: PathBuf,
    ) -> Self {
        Self {
            inner,
            interceptor,
            in_flight,
            inode_map: InodePathMap::new(root_dir),
        }
    }

    /// Convert a `codeagent_common::CodeAgentError` to `io::Error` with EACCES.
    fn interceptor_error_to_io(err: codeagent_common::CodeAgentError) -> io::Error {
        io::Error::new(io::ErrorKind::PermissionDenied, err.to_string())
    }

    /// Resolve an inode to its host path.
    fn resolve_path(&self, inode: u64) -> io::Result<PathBuf> {
        self.inode_map.get(inode)
    }

    /// Resolve parent inode + child name to host path.
    fn resolve_child_path(&self, parent: u64, name: &CStr) -> io::Result<PathBuf> {
        self.inode_map.resolve(parent, name)
    }
}

/// O_TRUNC flag value (matches Linux kernel definition).
const O_TRUNC: u32 = 0o1000;

impl FileSystem for InterceptedFs {
    type Inode = <PassthroughFs as FileSystem>::Inode;
    type Handle = <PassthroughFs as FileSystem>::Handle;
    type DirIter = <PassthroughFs as FileSystem>::DirIter;

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    fn init(&self, capable: FsOptions) -> io::Result<FsOptions> {
        self.inner.init(capable)
    }

    fn destroy(&self) {
        self.inner.destroy()
    }

    // -----------------------------------------------------------------------
    // Read-only methods (delegated directly)
    // -----------------------------------------------------------------------

    fn statfs(&self, ctx: Context, inode: Self::Inode) -> io::Result<libc::statvfs64> {
        self.inner.statfs(ctx, inode)
    }

    fn getattr(
        &self,
        ctx: Context,
        inode: Self::Inode,
        handle: Option<Self::Handle>,
    ) -> io::Result<(Attr, Duration)> {
        self.inner.getattr(ctx, inode, handle)
    }

    fn readlink(&self, ctx: Context, inode: Self::Inode) -> io::Result<Vec<u8>> {
        self.inner.readlink(ctx, inode)
    }

    fn open(
        &self,
        ctx: Context,
        inode: Self::Inode,
        kill_priv: bool,
        flags: u32,
    ) -> io::Result<(Option<Self::Handle>, OpenOptions)> {
        // If O_TRUNC is set, this is a mutating operation.
        if flags & O_TRUNC != 0 {
            let _guard = InFlightGuard::new(&self.in_flight);
            if let Ok(path) = self.resolve_path(inode) {
                self.interceptor
                    .pre_open_trunc(&path)
                    .map_err(Self::interceptor_error_to_io)?;
            }
            return self.inner.open(ctx, inode, kill_priv, flags);
        }
        self.inner.open(ctx, inode, kill_priv, flags)
    }

    fn read<W: ZeroCopyWriter>(
        &self,
        ctx: Context,
        inode: Self::Inode,
        handle: Self::Handle,
        w: W,
        size: u32,
        offset: u64,
        lock_owner: Option<u64>,
        flags: u32,
    ) -> io::Result<usize> {
        self.inner
            .read(ctx, inode, handle, w, size, offset, lock_owner, flags)
    }

    fn flush(
        &self,
        ctx: Context,
        inode: Self::Inode,
        handle: Self::Handle,
        lock_owner: u64,
    ) -> io::Result<()> {
        self.inner.flush(ctx, inode, handle, lock_owner)
    }

    fn release(
        &self,
        ctx: Context,
        inode: Self::Inode,
        flags: u32,
        handle: Self::Handle,
        flush: bool,
        flock_release: bool,
        lock_owner: Option<u64>,
    ) -> io::Result<()> {
        self.inner
            .release(ctx, inode, flags, handle, flush, flock_release, lock_owner)
    }

    fn fsync(
        &self,
        ctx: Context,
        inode: Self::Inode,
        datasync: bool,
        handle: Self::Handle,
    ) -> io::Result<()> {
        self.inner.fsync(ctx, inode, datasync, handle)
    }

    fn opendir(
        &self,
        ctx: Context,
        inode: Self::Inode,
        flags: u32,
    ) -> io::Result<(Option<Self::Handle>, OpenOptions)> {
        self.inner.opendir(ctx, inode, flags)
    }

    fn readdir(
        &self,
        ctx: Context,
        inode: Self::Inode,
        handle: Self::Handle,
        size: u32,
        offset: u64,
    ) -> io::Result<Self::DirIter> {
        self.inner.readdir(ctx, inode, handle, size, offset)
    }

    fn readdirplus(
        &self,
        ctx: Context,
        inode: Self::Inode,
        handle: Self::Handle,
        size: u32,
        offset: u64,
    ) -> io::Result<Self::DirIter> {
        self.inner.readdirplus(ctx, inode, handle, size, offset)
    }

    fn releasedir(
        &self,
        ctx: Context,
        inode: Self::Inode,
        flags: u32,
        handle: Self::Handle,
    ) -> io::Result<()> {
        self.inner.releasedir(ctx, inode, flags, handle)
    }

    fn fsyncdir(
        &self,
        ctx: Context,
        inode: Self::Inode,
        datasync: bool,
        handle: Self::Handle,
    ) -> io::Result<()> {
        self.inner.fsyncdir(ctx, inode, datasync, handle)
    }

    fn getxattr(
        &self,
        ctx: Context,
        inode: Self::Inode,
        name: &CStr,
        size: u32,
    ) -> io::Result<GetxattrReply> {
        self.inner.getxattr(ctx, inode, name, size)
    }

    fn listxattr(
        &self,
        ctx: Context,
        inode: Self::Inode,
        size: u32,
    ) -> io::Result<ListxattrReply> {
        self.inner.listxattr(ctx, inode, size)
    }

    fn access(&self, ctx: Context, inode: Self::Inode, mask: u32) -> io::Result<()> {
        self.inner.access(ctx, inode, mask)
    }

    fn lseek(
        &self,
        ctx: Context,
        inode: Self::Inode,
        handle: Self::Handle,
        offset: u64,
        whence: u32,
    ) -> io::Result<u64> {
        self.inner.lseek(ctx, inode, handle, offset, whence)
    }

    // -----------------------------------------------------------------------
    // Lookup and forget — read-only but update inode_map
    // -----------------------------------------------------------------------

    fn lookup(&self, ctx: Context, parent: Self::Inode, name: &CStr) -> io::Result<Entry> {
        let entry = self.inner.lookup(ctx, parent, name)?;
        if entry.inode != 0 {
            if let Ok(path) = self.resolve_child_path(parent, name) {
                self.inode_map.insert(entry.inode, path);
            }
        }
        Ok(entry)
    }

    fn forget(&self, ctx: Context, inode: Self::Inode, count: u64) {
        self.inode_map.remove(inode);
        self.inner.forget(ctx, inode, count)
    }

    fn batch_forget(&self, ctx: Context, requests: Vec<(Self::Inode, u64)>) {
        for &(inode, _) in &requests {
            self.inode_map.remove(inode);
        }
        self.inner.batch_forget(ctx, requests)
    }

    // -----------------------------------------------------------------------
    // Mutating methods — WriteInterceptor hooks + InFlightTracker
    // -----------------------------------------------------------------------

    fn write<R: ZeroCopyReader>(
        &self,
        ctx: Context,
        inode: Self::Inode,
        handle: Self::Handle,
        r: R,
        size: u32,
        offset: u64,
        lock_owner: Option<u64>,
        delayed_write: bool,
        kill_priv: bool,
        flags: u32,
    ) -> io::Result<usize> {
        let _guard = InFlightGuard::new(&self.in_flight);
        if let Ok(path) = self.resolve_path(inode) {
            self.interceptor
                .pre_write(&path)
                .map_err(Self::interceptor_error_to_io)?;
        }
        self.inner.write(
            ctx,
            inode,
            handle,
            r,
            size,
            offset,
            lock_owner,
            delayed_write,
            kill_priv,
            flags,
        )
    }

    fn create(
        &self,
        ctx: Context,
        parent: Self::Inode,
        name: &CStr,
        mode: u32,
        kill_priv: bool,
        flags: u32,
        umask: u32,
        extensions: Extensions,
    ) -> io::Result<(Entry, Option<Self::Handle>, OpenOptions)> {
        let _guard = InFlightGuard::new(&self.in_flight);
        let child_path = self.resolve_child_path(parent, name)?;

        // If the file exists and O_TRUNC is set, it's an overwrite.
        let file_existed = child_path.exists();
        if file_existed && flags & O_TRUNC != 0 {
            self.interceptor
                .pre_open_trunc(&child_path)
                .map_err(Self::interceptor_error_to_io)?;
        }

        let (entry, handle, opts) =
            self.inner
                .create(ctx, parent, name, mode, kill_priv, flags, umask, extensions)?;

        if entry.inode != 0 {
            self.inode_map.insert(entry.inode, child_path.clone());
        }

        if !file_existed {
            let _ = self.interceptor.post_create(&child_path);
        }

        Ok((entry, handle, opts))
    }

    fn mkdir(
        &self,
        ctx: Context,
        parent: Self::Inode,
        name: &CStr,
        mode: u32,
        umask: u32,
        extensions: Extensions,
    ) -> io::Result<Entry> {
        let _guard = InFlightGuard::new(&self.in_flight);
        let entry = self
            .inner
            .mkdir(ctx, parent, name, mode, umask, extensions)?;
        if let Ok(path) = self.resolve_child_path(parent, name) {
            if entry.inode != 0 {
                self.inode_map.insert(entry.inode, path.clone());
            }
            let _ = self.interceptor.post_mkdir(&path);
        }
        Ok(entry)
    }

    fn mknod(
        &self,
        ctx: Context,
        parent: Self::Inode,
        name: &CStr,
        mode: u32,
        rdev: u32,
        umask: u32,
        extensions: Extensions,
    ) -> io::Result<Entry> {
        let _guard = InFlightGuard::new(&self.in_flight);
        let entry = self
            .inner
            .mknod(ctx, parent, name, mode, rdev, umask, extensions)?;
        if let Ok(path) = self.resolve_child_path(parent, name) {
            if entry.inode != 0 {
                self.inode_map.insert(entry.inode, path.clone());
            }
            let _ = self.interceptor.post_create(&path);
        }
        Ok(entry)
    }

    fn unlink(&self, ctx: Context, parent: Self::Inode, name: &CStr) -> io::Result<()> {
        let _guard = InFlightGuard::new(&self.in_flight);
        if let Ok(path) = self.resolve_child_path(parent, name) {
            self.interceptor
                .pre_unlink(&path, false)
                .map_err(Self::interceptor_error_to_io)?;
        }
        self.inner.unlink(ctx, parent, name)?;
        // Inode is cleaned up via `forget` when the kernel evicts it from cache.
        Ok(())
    }

    fn rmdir(&self, ctx: Context, parent: Self::Inode, name: &CStr) -> io::Result<()> {
        let _guard = InFlightGuard::new(&self.in_flight);
        if let Ok(path) = self.resolve_child_path(parent, name) {
            self.interceptor
                .pre_unlink(&path, true)
                .map_err(Self::interceptor_error_to_io)?;
        }
        self.inner.rmdir(ctx, parent, name)?;
        Ok(())
    }

    fn rename(
        &self,
        ctx: Context,
        olddir: Self::Inode,
        oldname: &CStr,
        newdir: Self::Inode,
        newname: &CStr,
        flags: u32,
    ) -> io::Result<()> {
        let _guard = InFlightGuard::new(&self.in_flight);
        let old_path = self.resolve_child_path(olddir, oldname)?;
        let new_path = self.resolve_child_path(newdir, newname)?;

        self.interceptor
            .pre_rename(&old_path, &new_path)
            .map_err(Self::interceptor_error_to_io)?;

        self.inner
            .rename(ctx, olddir, oldname, newdir, newname, flags)?;
        let _ = self.inode_map.rename(olddir, oldname, newdir, newname);
        Ok(())
    }

    fn setattr(
        &self,
        ctx: Context,
        inode: Self::Inode,
        attr: SetattrIn,
        handle: Option<Self::Handle>,
        valid: SetattrValid,
    ) -> io::Result<(Attr, Duration)> {
        let _guard = InFlightGuard::new(&self.in_flight);
        if let Ok(path) = self.resolve_path(inode) {
            self.interceptor
                .pre_setattr(&path)
                .map_err(Self::interceptor_error_to_io)?;
        }
        self.inner.setattr(ctx, inode, attr, handle, valid)
    }

    fn symlink(
        &self,
        ctx: Context,
        linkname: &CStr,
        parent: Self::Inode,
        name: &CStr,
        extensions: Extensions,
    ) -> io::Result<Entry> {
        let _guard = InFlightGuard::new(&self.in_flight);
        let entry = self
            .inner
            .symlink(ctx, linkname, parent, name, extensions)?;
        if let Ok(link_path) = self.resolve_child_path(parent, name) {
            if entry.inode != 0 {
                self.inode_map.insert(entry.inode, link_path.clone());
            }
            let target = linkname.to_str().unwrap_or_default();
            let target_path = std::path::Path::new(target);
            let _ = self.interceptor.post_symlink(target_path, &link_path);
        }
        Ok(entry)
    }

    fn link(
        &self,
        ctx: Context,
        inode: Self::Inode,
        newparent: Self::Inode,
        newname: &CStr,
    ) -> io::Result<Entry> {
        let _guard = InFlightGuard::new(&self.in_flight);
        let target_path = self.resolve_path(inode)?;
        let link_path = self.resolve_child_path(newparent, newname)?;

        self.interceptor
            .pre_link(&target_path, &link_path)
            .map_err(Self::interceptor_error_to_io)?;

        let entry = self.inner.link(ctx, inode, newparent, newname)?;
        if entry.inode != 0 {
            self.inode_map.insert(entry.inode, link_path);
        }
        Ok(entry)
    }

    fn fallocate(
        &self,
        ctx: Context,
        inode: Self::Inode,
        handle: Self::Handle,
        mode: u32,
        offset: u64,
        length: u64,
    ) -> io::Result<()> {
        let _guard = InFlightGuard::new(&self.in_flight);
        if let Ok(path) = self.resolve_path(inode) {
            self.interceptor
                .pre_fallocate(&path)
                .map_err(Self::interceptor_error_to_io)?;
        }
        self.inner
            .fallocate(ctx, inode, handle, mode, offset, length)
    }

    fn setxattr(
        &self,
        ctx: Context,
        inode: Self::Inode,
        name: &CStr,
        value: &[u8],
        flags: u32,
        extra_flags: SetxattrFlags,
    ) -> io::Result<()> {
        let _guard = InFlightGuard::new(&self.in_flight);
        if let Ok(path) = self.resolve_path(inode) {
            self.interceptor
                .pre_xattr(&path)
                .map_err(Self::interceptor_error_to_io)?;
        }
        self.inner
            .setxattr(ctx, inode, name, value, flags, extra_flags)
    }

    fn removexattr(&self, ctx: Context, inode: Self::Inode, name: &CStr) -> io::Result<()> {
        let _guard = InFlightGuard::new(&self.in_flight);
        if let Ok(path) = self.resolve_path(inode) {
            self.interceptor
                .pre_xattr(&path)
                .map_err(Self::interceptor_error_to_io)?;
        }
        self.inner.removexattr(ctx, inode, name)
    }

    fn copyfilerange(
        &self,
        ctx: Context,
        inode_in: Self::Inode,
        handle_in: Self::Handle,
        offset_in: u64,
        inode_out: Self::Inode,
        handle_out: Self::Handle,
        offset_out: u64,
        len: u64,
        flags: u64,
    ) -> io::Result<usize> {
        let _guard = InFlightGuard::new(&self.in_flight);
        if let Ok(dst_path) = self.resolve_path(inode_out) {
            self.interceptor
                .pre_copy_file_range(&dst_path)
                .map_err(Self::interceptor_error_to_io)?;
        }
        self.inner.copyfilerange(
            ctx, inode_in, handle_in, offset_in, inode_out, handle_out, offset_out, len, flags,
        )
    }

    fn tmpfile(
        &self,
        ctx: Context,
        parent: Self::Inode,
        mode: u32,
        flags: u32,
        umask: u32,
    ) -> io::Result<(Entry, Option<Self::Handle>, OpenOptions)> {
        let _guard = InFlightGuard::new(&self.in_flight);
        self.inner.tmpfile(ctx, parent, mode, flags, umask)
    }

    fn syncfs(&self, ctx: Context, inode: Self::Inode) -> io::Result<()> {
        self.inner.syncfs(ctx, inode)
    }
}

impl SerializableFileSystem for InterceptedFs {
    fn prepare_serialization(&self, cancel: Arc<std::sync::atomic::AtomicBool>) {
        self.inner.prepare_serialization(cancel)
    }

    fn serialize(&self, state_pipe: std::fs::File) -> io::Result<()> {
        self.inner.serialize(state_pipe)
    }

    fn deserialize_and_apply(&self, state_pipe: std::fs::File) -> io::Result<()> {
        self.inner.deserialize_and_apply(state_pipe)
    }
}
