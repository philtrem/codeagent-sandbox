use crate::error::P9Error;
use crate::qid::Qid;
use crate::wire::{WireReader, WireWriter};

// ---------------------------------------------------------------------------
// Message type constants
// ---------------------------------------------------------------------------

/// Error response (9P2000.L only — carries a Linux errno).
pub const RLERROR: u8 = 7;

pub const TSTATFS: u8 = 8;
pub const RSTATFS: u8 = 9;
pub const TLOPEN: u8 = 12;
pub const RLOPEN: u8 = 13;
pub const TLCREATE: u8 = 14;
pub const RLCREATE: u8 = 15;
pub const TSYMLINK: u8 = 16;
pub const RSYMLINK: u8 = 17;
pub const TMKNOD: u8 = 18;
pub const RMKNOD: u8 = 19;
pub const TRENAME: u8 = 20;
pub const RRENAME: u8 = 21;
pub const TREADLINK: u8 = 22;
pub const RREADLINK: u8 = 23;
pub const TGETATTR: u8 = 24;
pub const RGETATTR: u8 = 25;
pub const TSETATTR: u8 = 26;
pub const RSETATTR: u8 = 27;
pub const TXATTRWALK: u8 = 30;
pub const RXATTRWALK: u8 = 31;
pub const TXATTRCREATE: u8 = 32;
pub const RXATTRCREATE: u8 = 33;
pub const TREADDIR: u8 = 40;
pub const RREADDIR: u8 = 41;
pub const TFSYNC: u8 = 50;
pub const RFSYNC: u8 = 51;
pub const TLOCK: u8 = 52;
pub const RLOCK: u8 = 53;
pub const TGETLOCK: u8 = 54;
pub const RGETLOCK: u8 = 55;
pub const TLINK: u8 = 70;
pub const RLINK: u8 = 71;
pub const TMKDIR: u8 = 72;
pub const RMKDIR: u8 = 73;
pub const TRENAMEAT: u8 = 74;
pub const RRENAMEAT: u8 = 75;
pub const TUNLINKAT: u8 = 76;
pub const RUNLINKAT: u8 = 77;

// Core 9P2000 messages
pub const TVERSION: u8 = 100;
pub const RVERSION: u8 = 101;
pub const TAUTH: u8 = 102;
pub const RAUTH: u8 = 103;
pub const TATTACH: u8 = 104;
pub const RATTACH: u8 = 105;
// 110 = Terror (invalid, never sent)
pub const TFLUSH: u8 = 108;
pub const RFLUSH: u8 = 109;
pub const TWALK: u8 = 110;
pub const RWALK: u8 = 111;
pub const TREAD: u8 = 116;
pub const RREAD: u8 = 117;
pub const TWRITE: u8 = 118;
pub const RWRITE: u8 = 119;
pub const TCLUNK: u8 = 120;
pub const RCLUNK: u8 = 121;
pub const TREMOVE: u8 = 122;
pub const RREMOVE: u8 = 123;

/// Special tag value used only for Tversion messages.
pub const NOTAG: u16 = 0xFFFF;

// ---------------------------------------------------------------------------
// Getattr mask constants
// ---------------------------------------------------------------------------

pub const P9_GETATTR_MODE: u64 = 0x00000001;
pub const P9_GETATTR_NLINK: u64 = 0x00000002;
pub const P9_GETATTR_UID: u64 = 0x00000004;
pub const P9_GETATTR_GID: u64 = 0x00000008;
pub const P9_GETATTR_RDEV: u64 = 0x00000010;
pub const P9_GETATTR_ATIME: u64 = 0x00000020;
pub const P9_GETATTR_MTIME: u64 = 0x00000040;
pub const P9_GETATTR_CTIME: u64 = 0x00000080;
pub const P9_GETATTR_INO: u64 = 0x00000100;
pub const P9_GETATTR_SIZE: u64 = 0x00000200;
pub const P9_GETATTR_BLOCKS: u64 = 0x00000400;
pub const P9_GETATTR_BTIME: u64 = 0x00000800;
pub const P9_GETATTR_GEN: u64 = 0x00001000;
pub const P9_GETATTR_DATA_VERSION: u64 = 0x00002000;
pub const P9_GETATTR_BASIC: u64 = 0x000007ff;
pub const P9_GETATTR_ALL: u64 = 0x00003fff;

// ---------------------------------------------------------------------------
// Setattr mask constants
// ---------------------------------------------------------------------------

pub const P9_SETATTR_MODE: u32 = 0x00000001;
pub const P9_SETATTR_UID: u32 = 0x00000002;
pub const P9_SETATTR_GID: u32 = 0x00000004;
pub const P9_SETATTR_SIZE: u32 = 0x00000008;
pub const P9_SETATTR_ATIME: u32 = 0x00000010;
pub const P9_SETATTR_MTIME: u32 = 0x00000020;
pub const P9_SETATTR_CTIME: u32 = 0x00000040;
pub const P9_SETATTR_ATIME_SET: u32 = 0x00000100;
pub const P9_SETATTR_MTIME_SET: u32 = 0x00000200;

// ---------------------------------------------------------------------------
// Unlinkat flags
// ---------------------------------------------------------------------------

/// AT_REMOVEDIR flag for Tunlinkat (indicates directory removal).
pub const AT_REMOVEDIR: u32 = 0x200;

// ---------------------------------------------------------------------------
// Lock types and status
// ---------------------------------------------------------------------------

pub const P9_LOCK_TYPE_RDLCK: u8 = 0;
pub const P9_LOCK_TYPE_WRLCK: u8 = 1;
pub const P9_LOCK_TYPE_UNLCK: u8 = 2;

pub const P9_LOCK_SUCCESS: u8 = 0;
pub const P9_LOCK_BLOCKED: u8 = 1;
pub const P9_LOCK_ERROR: u8 = 2;
pub const P9_LOCK_GRACE: u8 = 3;

pub const P9_LOCK_FLAGS_BLOCK: u32 = 1;

// ---------------------------------------------------------------------------
// Readdir entry type constants (matching Linux dirent d_type)
// ---------------------------------------------------------------------------

pub const DT_UNKNOWN: u8 = 0;
pub const DT_FIFO: u8 = 1;
pub const DT_CHR: u8 = 2;
pub const DT_DIR: u8 = 4;
pub const DT_BLK: u8 = 6;
pub const DT_REG: u8 = 8;
pub const DT_LNK: u8 = 10;
pub const DT_SOCK: u8 = 12;

// ---------------------------------------------------------------------------
// Message trait
// ---------------------------------------------------------------------------

/// Trait implemented by all 9P message types for wire serialization.
pub trait Message: Sized {
    /// The message type byte (e.g., `TVERSION`, `RVERSION`).
    const MSG_TYPE: u8;

    /// Decode the message body from a wire reader.
    /// The reader is positioned after the type and tag bytes.
    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error>;

    /// Encode the message body to a wire writer.
    /// The caller is responsible for writing the type and tag bytes.
    fn encode(&self, writer: &mut WireWriter);

    /// Build a complete wire message (size + type + tag + body).
    fn to_wire(&self, tag: u16) -> Vec<u8> {
        let mut writer = WireWriter::new();
        writer.write_u8(Self::MSG_TYPE);
        writer.write_u16(tag);
        self.encode(&mut writer);
        writer.finish()
    }
}

// ---------------------------------------------------------------------------
// Core 9P2000 messages
// ---------------------------------------------------------------------------

// -- Rlerror --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rlerror {
    pub ecode: u32,
}

impl Message for Rlerror {
    const MSG_TYPE: u8 = RLERROR;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            ecode: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.ecode);
    }
}

// -- Tversion / Rversion --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tversion {
    pub msize: u32,
    pub version: String,
}

impl Message for Tversion {
    const MSG_TYPE: u8 = TVERSION;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            msize: reader.read_u32()?,
            version: reader.read_string()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.msize);
        writer.write_string(&self.version);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rversion {
    pub msize: u32,
    pub version: String,
}

impl Message for Rversion {
    const MSG_TYPE: u8 = RVERSION;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            msize: reader.read_u32()?,
            version: reader.read_string()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.msize);
        writer.write_string(&self.version);
    }
}

// -- Tauth / Rauth --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tauth {
    pub afid: u32,
    pub uname: String,
    pub aname: String,
    pub n_uname: u32,
}

impl Message for Tauth {
    const MSG_TYPE: u8 = TAUTH;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            afid: reader.read_u32()?,
            uname: reader.read_string()?,
            aname: reader.read_string()?,
            n_uname: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.afid);
        writer.write_string(&self.uname);
        writer.write_string(&self.aname);
        writer.write_u32(self.n_uname);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rauth {
    pub aqid: Qid,
}

impl Message for Rauth {
    const MSG_TYPE: u8 = RAUTH;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            aqid: reader.read_qid()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_qid(&self.aqid);
    }
}

// -- Tattach / Rattach --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tattach {
    pub fid: u32,
    pub afid: u32,
    pub uname: String,
    pub aname: String,
    pub n_uname: u32,
}

impl Message for Tattach {
    const MSG_TYPE: u8 = TATTACH;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fid: reader.read_u32()?,
            afid: reader.read_u32()?,
            uname: reader.read_string()?,
            aname: reader.read_string()?,
            n_uname: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
        writer.write_u32(self.afid);
        writer.write_string(&self.uname);
        writer.write_string(&self.aname);
        writer.write_u32(self.n_uname);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rattach {
    pub qid: Qid,
}

impl Message for Rattach {
    const MSG_TYPE: u8 = RATTACH;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            qid: reader.read_qid()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_qid(&self.qid);
    }
}

// -- Tflush / Rflush --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tflush {
    pub oldtag: u16,
}

impl Message for Tflush {
    const MSG_TYPE: u8 = TFLUSH;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            oldtag: reader.read_u16()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u16(self.oldtag);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rflush;

impl Message for Rflush {
    const MSG_TYPE: u8 = RFLUSH;

    fn decode(_reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self)
    }

    fn encode(&self, _writer: &mut WireWriter) {}
}

// -- Twalk / Rwalk --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Twalk {
    pub fid: u32,
    pub newfid: u32,
    pub wnames: Vec<String>,
}

impl Message for Twalk {
    const MSG_TYPE: u8 = TWALK;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        let fid = reader.read_u32()?;
        let newfid = reader.read_u32()?;
        let wnames = reader.read_strings()?;
        Ok(Self {
            fid,
            newfid,
            wnames,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
        writer.write_u32(self.newfid);
        writer.write_strings(&self.wnames);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rwalk {
    pub wqids: Vec<Qid>,
}

impl Message for Rwalk {
    const MSG_TYPE: u8 = RWALK;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            wqids: reader.read_qids()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_qids(&self.wqids);
    }
}

// -- Tread / Rread --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tread {
    pub fid: u32,
    pub offset: u64,
    pub count: u32,
}

impl Message for Tread {
    const MSG_TYPE: u8 = TREAD;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fid: reader.read_u32()?,
            offset: reader.read_u64()?,
            count: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
        writer.write_u64(self.offset);
        writer.write_u32(self.count);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rread {
    pub data: Vec<u8>,
}

impl Message for Rread {
    const MSG_TYPE: u8 = RREAD;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            data: reader.read_data()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_data(&self.data);
    }
}

// -- Twrite / Rwrite --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Twrite {
    pub fid: u32,
    pub offset: u64,
    pub data: Vec<u8>,
}

impl Message for Twrite {
    const MSG_TYPE: u8 = TWRITE;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fid: reader.read_u32()?,
            offset: reader.read_u64()?,
            data: reader.read_data()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
        writer.write_u64(self.offset);
        writer.write_data(&self.data);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rwrite {
    pub count: u32,
}

impl Message for Rwrite {
    const MSG_TYPE: u8 = RWRITE;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            count: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.count);
    }
}

// -- Tclunk / Rclunk --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tclunk {
    pub fid: u32,
}

impl Message for Tclunk {
    const MSG_TYPE: u8 = TCLUNK;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fid: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rclunk;

impl Message for Rclunk {
    const MSG_TYPE: u8 = RCLUNK;

    fn decode(_reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self)
    }

    fn encode(&self, _writer: &mut WireWriter) {}
}

// -- Tremove / Rremove --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tremove {
    pub fid: u32,
}

impl Message for Tremove {
    const MSG_TYPE: u8 = TREMOVE;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fid: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rremove;

impl Message for Rremove {
    const MSG_TYPE: u8 = RREMOVE;

    fn decode(_reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self)
    }

    fn encode(&self, _writer: &mut WireWriter) {}
}

// ---------------------------------------------------------------------------
// 9P2000.L extension messages
// ---------------------------------------------------------------------------

// -- Tstatfs / Rstatfs --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tstatfs {
    pub fid: u32,
}

impl Message for Tstatfs {
    const MSG_TYPE: u8 = TSTATFS;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fid: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rstatfs {
    pub fs_type: u32,
    pub bsize: u32,
    pub blocks: u64,
    pub bfree: u64,
    pub bavail: u64,
    pub files: u64,
    pub ffree: u64,
    pub fsid: u64,
    pub namelen: u32,
}

impl Message for Rstatfs {
    const MSG_TYPE: u8 = RSTATFS;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fs_type: reader.read_u32()?,
            bsize: reader.read_u32()?,
            blocks: reader.read_u64()?,
            bfree: reader.read_u64()?,
            bavail: reader.read_u64()?,
            files: reader.read_u64()?,
            ffree: reader.read_u64()?,
            fsid: reader.read_u64()?,
            namelen: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fs_type);
        writer.write_u32(self.bsize);
        writer.write_u64(self.blocks);
        writer.write_u64(self.bfree);
        writer.write_u64(self.bavail);
        writer.write_u64(self.files);
        writer.write_u64(self.ffree);
        writer.write_u64(self.fsid);
        writer.write_u32(self.namelen);
    }
}

// -- Tlopen / Rlopen --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tlopen {
    pub fid: u32,
    pub flags: u32,
}

impl Message for Tlopen {
    const MSG_TYPE: u8 = TLOPEN;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fid: reader.read_u32()?,
            flags: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
        writer.write_u32(self.flags);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rlopen {
    pub qid: Qid,
    pub iounit: u32,
}

impl Message for Rlopen {
    const MSG_TYPE: u8 = RLOPEN;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            qid: reader.read_qid()?,
            iounit: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_qid(&self.qid);
        writer.write_u32(self.iounit);
    }
}

// -- Tlcreate / Rlcreate --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tlcreate {
    pub fid: u32,
    pub name: String,
    pub flags: u32,
    pub mode: u32,
    pub gid: u32,
}

impl Message for Tlcreate {
    const MSG_TYPE: u8 = TLCREATE;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fid: reader.read_u32()?,
            name: reader.read_string()?,
            flags: reader.read_u32()?,
            mode: reader.read_u32()?,
            gid: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
        writer.write_string(&self.name);
        writer.write_u32(self.flags);
        writer.write_u32(self.mode);
        writer.write_u32(self.gid);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rlcreate {
    pub qid: Qid,
    pub iounit: u32,
}

impl Message for Rlcreate {
    const MSG_TYPE: u8 = RLCREATE;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            qid: reader.read_qid()?,
            iounit: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_qid(&self.qid);
        writer.write_u32(self.iounit);
    }
}

// -- Tsymlink / Rsymlink --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tsymlink {
    pub fid: u32,
    pub name: String,
    pub symtgt: String,
    pub gid: u32,
}

impl Message for Tsymlink {
    const MSG_TYPE: u8 = TSYMLINK;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fid: reader.read_u32()?,
            name: reader.read_string()?,
            symtgt: reader.read_string()?,
            gid: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
        writer.write_string(&self.name);
        writer.write_string(&self.symtgt);
        writer.write_u32(self.gid);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rsymlink {
    pub qid: Qid,
}

impl Message for Rsymlink {
    const MSG_TYPE: u8 = RSYMLINK;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            qid: reader.read_qid()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_qid(&self.qid);
    }
}

// -- Tmknod / Rmknod --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tmknod {
    pub dfid: u32,
    pub name: String,
    pub mode: u32,
    pub major: u32,
    pub minor: u32,
    pub gid: u32,
}

impl Message for Tmknod {
    const MSG_TYPE: u8 = TMKNOD;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            dfid: reader.read_u32()?,
            name: reader.read_string()?,
            mode: reader.read_u32()?,
            major: reader.read_u32()?,
            minor: reader.read_u32()?,
            gid: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.dfid);
        writer.write_string(&self.name);
        writer.write_u32(self.mode);
        writer.write_u32(self.major);
        writer.write_u32(self.minor);
        writer.write_u32(self.gid);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rmknod {
    pub qid: Qid,
}

impl Message for Rmknod {
    const MSG_TYPE: u8 = RMKNOD;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            qid: reader.read_qid()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_qid(&self.qid);
    }
}

// -- Trename / Rrename --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trename {
    pub fid: u32,
    pub dfid: u32,
    pub name: String,
}

impl Message for Trename {
    const MSG_TYPE: u8 = TRENAME;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fid: reader.read_u32()?,
            dfid: reader.read_u32()?,
            name: reader.read_string()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
        writer.write_u32(self.dfid);
        writer.write_string(&self.name);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rrename;

impl Message for Rrename {
    const MSG_TYPE: u8 = RRENAME;

    fn decode(_reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self)
    }

    fn encode(&self, _writer: &mut WireWriter) {}
}

// -- Treadlink / Rreadlink --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Treadlink {
    pub fid: u32,
}

impl Message for Treadlink {
    const MSG_TYPE: u8 = TREADLINK;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fid: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rreadlink {
    pub target: String,
}

impl Message for Rreadlink {
    const MSG_TYPE: u8 = RREADLINK;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            target: reader.read_string()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_string(&self.target);
    }
}

// -- Tgetattr / Rgetattr --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tgetattr {
    pub fid: u32,
    pub request_mask: u64,
}

impl Message for Tgetattr {
    const MSG_TYPE: u8 = TGETATTR;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fid: reader.read_u32()?,
            request_mask: reader.read_u64()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
        writer.write_u64(self.request_mask);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgetattr {
    pub valid: u64,
    pub qid: Qid,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u64,
    pub rdev: u64,
    pub size: u64,
    pub blksize: u64,
    pub blocks: u64,
    pub atime_sec: u64,
    pub atime_nsec: u64,
    pub mtime_sec: u64,
    pub mtime_nsec: u64,
    pub ctime_sec: u64,
    pub ctime_nsec: u64,
    pub btime_sec: u64,
    pub btime_nsec: u64,
    pub generation: u64,
    pub data_version: u64,
}

impl Message for Rgetattr {
    const MSG_TYPE: u8 = RGETATTR;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            valid: reader.read_u64()?,
            qid: reader.read_qid()?,
            mode: reader.read_u32()?,
            uid: reader.read_u32()?,
            gid: reader.read_u32()?,
            nlink: reader.read_u64()?,
            rdev: reader.read_u64()?,
            size: reader.read_u64()?,
            blksize: reader.read_u64()?,
            blocks: reader.read_u64()?,
            atime_sec: reader.read_u64()?,
            atime_nsec: reader.read_u64()?,
            mtime_sec: reader.read_u64()?,
            mtime_nsec: reader.read_u64()?,
            ctime_sec: reader.read_u64()?,
            ctime_nsec: reader.read_u64()?,
            btime_sec: reader.read_u64()?,
            btime_nsec: reader.read_u64()?,
            generation: reader.read_u64()?,
            data_version: reader.read_u64()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u64(self.valid);
        writer.write_qid(&self.qid);
        writer.write_u32(self.mode);
        writer.write_u32(self.uid);
        writer.write_u32(self.gid);
        writer.write_u64(self.nlink);
        writer.write_u64(self.rdev);
        writer.write_u64(self.size);
        writer.write_u64(self.blksize);
        writer.write_u64(self.blocks);
        writer.write_u64(self.atime_sec);
        writer.write_u64(self.atime_nsec);
        writer.write_u64(self.mtime_sec);
        writer.write_u64(self.mtime_nsec);
        writer.write_u64(self.ctime_sec);
        writer.write_u64(self.ctime_nsec);
        writer.write_u64(self.btime_sec);
        writer.write_u64(self.btime_nsec);
        writer.write_u64(self.generation);
        writer.write_u64(self.data_version);
    }
}

// -- Tsetattr / Rsetattr --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tsetattr {
    pub fid: u32,
    pub valid: u32,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub atime_sec: u64,
    pub atime_nsec: u64,
    pub mtime_sec: u64,
    pub mtime_nsec: u64,
}

impl Message for Tsetattr {
    const MSG_TYPE: u8 = TSETATTR;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fid: reader.read_u32()?,
            valid: reader.read_u32()?,
            mode: reader.read_u32()?,
            uid: reader.read_u32()?,
            gid: reader.read_u32()?,
            size: reader.read_u64()?,
            atime_sec: reader.read_u64()?,
            atime_nsec: reader.read_u64()?,
            mtime_sec: reader.read_u64()?,
            mtime_nsec: reader.read_u64()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
        writer.write_u32(self.valid);
        writer.write_u32(self.mode);
        writer.write_u32(self.uid);
        writer.write_u32(self.gid);
        writer.write_u64(self.size);
        writer.write_u64(self.atime_sec);
        writer.write_u64(self.atime_nsec);
        writer.write_u64(self.mtime_sec);
        writer.write_u64(self.mtime_nsec);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rsetattr;

impl Message for Rsetattr {
    const MSG_TYPE: u8 = RSETATTR;

    fn decode(_reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self)
    }

    fn encode(&self, _writer: &mut WireWriter) {}
}

// -- Txattrwalk / Rxattrwalk --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Txattrwalk {
    pub fid: u32,
    pub newfid: u32,
    pub name: String,
}

impl Message for Txattrwalk {
    const MSG_TYPE: u8 = TXATTRWALK;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fid: reader.read_u32()?,
            newfid: reader.read_u32()?,
            name: reader.read_string()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
        writer.write_u32(self.newfid);
        writer.write_string(&self.name);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rxattrwalk {
    pub size: u64,
}

impl Message for Rxattrwalk {
    const MSG_TYPE: u8 = RXATTRWALK;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            size: reader.read_u64()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u64(self.size);
    }
}

// -- Txattrcreate / Rxattrcreate --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Txattrcreate {
    pub fid: u32,
    pub name: String,
    pub attr_size: u64,
    pub flags: u32,
}

impl Message for Txattrcreate {
    const MSG_TYPE: u8 = TXATTRCREATE;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fid: reader.read_u32()?,
            name: reader.read_string()?,
            attr_size: reader.read_u64()?,
            flags: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
        writer.write_string(&self.name);
        writer.write_u64(self.attr_size);
        writer.write_u32(self.flags);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rxattrcreate;

impl Message for Rxattrcreate {
    const MSG_TYPE: u8 = RXATTRCREATE;

    fn decode(_reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self)
    }

    fn encode(&self, _writer: &mut WireWriter) {}
}

// -- Treaddir / Rreaddir --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Treaddir {
    pub fid: u32,
    pub offset: u64,
    pub count: u32,
}

impl Message for Treaddir {
    const MSG_TYPE: u8 = TREADDIR;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fid: reader.read_u32()?,
            offset: reader.read_u64()?,
            count: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
        writer.write_u64(self.offset);
        writer.write_u32(self.count);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rreaddir {
    pub data: Vec<u8>,
}

impl Message for Rreaddir {
    const MSG_TYPE: u8 = RREADDIR;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            data: reader.read_data()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_data(&self.data);
    }
}

// -- Tfsync / Rfsync --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tfsync {
    pub fid: u32,
}

impl Message for Tfsync {
    const MSG_TYPE: u8 = TFSYNC;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fid: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rfsync;

impl Message for Rfsync {
    const MSG_TYPE: u8 = RFSYNC;

    fn decode(_reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self)
    }

    fn encode(&self, _writer: &mut WireWriter) {}
}

// -- Tlock / Rlock --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tlock {
    pub fid: u32,
    pub lock_type: u8,
    pub flags: u32,
    pub start: u64,
    pub length: u64,
    pub proc_id: u32,
    pub client_id: String,
}

impl Message for Tlock {
    const MSG_TYPE: u8 = TLOCK;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fid: reader.read_u32()?,
            lock_type: reader.read_u8()?,
            flags: reader.read_u32()?,
            start: reader.read_u64()?,
            length: reader.read_u64()?,
            proc_id: reader.read_u32()?,
            client_id: reader.read_string()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
        writer.write_u8(self.lock_type);
        writer.write_u32(self.flags);
        writer.write_u64(self.start);
        writer.write_u64(self.length);
        writer.write_u32(self.proc_id);
        writer.write_string(&self.client_id);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rlock {
    pub status: u8,
}

impl Message for Rlock {
    const MSG_TYPE: u8 = RLOCK;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            status: reader.read_u8()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u8(self.status);
    }
}

// -- Tgetlock / Rgetlock --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tgetlock {
    pub fid: u32,
    pub lock_type: u8,
    pub start: u64,
    pub length: u64,
    pub proc_id: u32,
    pub client_id: String,
}

impl Message for Tgetlock {
    const MSG_TYPE: u8 = TGETLOCK;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            fid: reader.read_u32()?,
            lock_type: reader.read_u8()?,
            start: reader.read_u64()?,
            length: reader.read_u64()?,
            proc_id: reader.read_u32()?,
            client_id: reader.read_string()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.fid);
        writer.write_u8(self.lock_type);
        writer.write_u64(self.start);
        writer.write_u64(self.length);
        writer.write_u32(self.proc_id);
        writer.write_string(&self.client_id);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgetlock {
    pub lock_type: u8,
    pub start: u64,
    pub length: u64,
    pub proc_id: u32,
    pub client_id: String,
}

impl Message for Rgetlock {
    const MSG_TYPE: u8 = RGETLOCK;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            lock_type: reader.read_u8()?,
            start: reader.read_u64()?,
            length: reader.read_u64()?,
            proc_id: reader.read_u32()?,
            client_id: reader.read_string()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u8(self.lock_type);
        writer.write_u64(self.start);
        writer.write_u64(self.length);
        writer.write_u32(self.proc_id);
        writer.write_string(&self.client_id);
    }
}

// -- Tlink / Rlink --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tlink {
    pub dfid: u32,
    pub fid: u32,
    pub name: String,
}

impl Message for Tlink {
    const MSG_TYPE: u8 = TLINK;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            dfid: reader.read_u32()?,
            fid: reader.read_u32()?,
            name: reader.read_string()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.dfid);
        writer.write_u32(self.fid);
        writer.write_string(&self.name);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rlink;

impl Message for Rlink {
    const MSG_TYPE: u8 = RLINK;

    fn decode(_reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self)
    }

    fn encode(&self, _writer: &mut WireWriter) {}
}

// -- Tmkdir / Rmkdir --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tmkdir {
    pub dfid: u32,
    pub name: String,
    pub mode: u32,
    pub gid: u32,
}

impl Message for Tmkdir {
    const MSG_TYPE: u8 = TMKDIR;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            dfid: reader.read_u32()?,
            name: reader.read_string()?,
            mode: reader.read_u32()?,
            gid: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.dfid);
        writer.write_string(&self.name);
        writer.write_u32(self.mode);
        writer.write_u32(self.gid);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rmkdir {
    pub qid: Qid,
}

impl Message for Rmkdir {
    const MSG_TYPE: u8 = RMKDIR;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            qid: reader.read_qid()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_qid(&self.qid);
    }
}

// -- Trenameat / Rrenameat --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trenameat {
    pub olddirfid: u32,
    pub oldname: String,
    pub newdirfid: u32,
    pub newname: String,
}

impl Message for Trenameat {
    const MSG_TYPE: u8 = TRENAMEAT;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            olddirfid: reader.read_u32()?,
            oldname: reader.read_string()?,
            newdirfid: reader.read_u32()?,
            newname: reader.read_string()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.olddirfid);
        writer.write_string(&self.oldname);
        writer.write_u32(self.newdirfid);
        writer.write_string(&self.newname);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rrenameat;

impl Message for Rrenameat {
    const MSG_TYPE: u8 = RRENAMEAT;

    fn decode(_reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self)
    }

    fn encode(&self, _writer: &mut WireWriter) {}
}

// -- Tunlinkat / Runlinkat --

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tunlinkat {
    pub dirfid: u32,
    pub name: String,
    pub flags: u32,
}

impl Message for Tunlinkat {
    const MSG_TYPE: u8 = TUNLINKAT;

    fn decode(reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self {
            dirfid: reader.read_u32()?,
            name: reader.read_string()?,
            flags: reader.read_u32()?,
        })
    }

    fn encode(&self, writer: &mut WireWriter) {
        writer.write_u32(self.dirfid);
        writer.write_string(&self.name);
        writer.write_u32(self.flags);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runlinkat;

impl Message for Runlinkat {
    const MSG_TYPE: u8 = RUNLINKAT;

    fn decode(_reader: &mut WireReader<'_>) -> Result<Self, P9Error> {
        Ok(Self)
    }

    fn encode(&self, _writer: &mut WireWriter) {}
}

// ---------------------------------------------------------------------------
// Dispatch: parse a raw message body into the correct type
// ---------------------------------------------------------------------------

/// A parsed 9P message (either T-message or R-message) with its tag.
#[derive(Debug)]
pub struct ParsedMessage {
    pub tag: u16,
    pub body: MessageBody,
}

/// The body of a parsed 9P message, discriminated by type.
#[derive(Debug)]
pub enum MessageBody {
    Rlerror(Rlerror),
    Tversion(Tversion),
    Rversion(Rversion),
    Tauth(Tauth),
    Rauth(Rauth),
    Tattach(Tattach),
    Rattach(Rattach),
    Tflush(Tflush),
    Rflush(Rflush),
    Twalk(Twalk),
    Rwalk(Rwalk),
    Tread(Tread),
    Rread(Rread),
    Twrite(Twrite),
    Rwrite(Rwrite),
    Tclunk(Tclunk),
    Rclunk(Rclunk),
    Tremove(Tremove),
    Rremove(Rremove),
    Tstatfs(Tstatfs),
    Rstatfs(Rstatfs),
    Tlopen(Tlopen),
    Rlopen(Rlopen),
    Tlcreate(Tlcreate),
    Rlcreate(Rlcreate),
    Tsymlink(Tsymlink),
    Rsymlink(Rsymlink),
    Tmknod(Tmknod),
    Rmknod(Rmknod),
    Trename(Trename),
    Rrename(Rrename),
    Treadlink(Treadlink),
    Rreadlink(Rreadlink),
    Tgetattr(Tgetattr),
    Rgetattr(Rgetattr),
    Tsetattr(Tsetattr),
    Rsetattr(Rsetattr),
    Txattrwalk(Txattrwalk),
    Rxattrwalk(Rxattrwalk),
    Txattrcreate(Txattrcreate),
    Rxattrcreate(Rxattrcreate),
    Treaddir(Treaddir),
    Rreaddir(Rreaddir),
    Tfsync(Tfsync),
    Rfsync(Rfsync),
    Tlock(Tlock),
    Rlock(Rlock),
    Tgetlock(Tgetlock),
    Rgetlock(Rgetlock),
    Tlink(Tlink),
    Rlink(Rlink),
    Tmkdir(Tmkdir),
    Rmkdir(Rmkdir),
    Trenameat(Trenameat),
    Rrenameat(Rrenameat),
    Tunlinkat(Tunlinkat),
    Runlinkat(Runlinkat),
}

/// Parse a complete 9P message from raw bytes (including the 4-byte size prefix).
///
/// The size field is used only for validation — the caller is expected to have
/// already read exactly `size` bytes from the transport.
pub fn parse_message(buf: &[u8]) -> Result<ParsedMessage, P9Error> {
    if buf.len() < 7 {
        return Err(P9Error::MalformedMessage {
            reason: format!(
                "message too short: {} bytes (minimum 7)",
                buf.len()
            ),
        });
    }

    let size = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if size as usize != buf.len() {
        return Err(P9Error::MalformedMessage {
            reason: format!(
                "size field ({size}) does not match buffer length ({})",
                buf.len()
            ),
        });
    }

    let msg_type = buf[4];
    let tag = u16::from_le_bytes([buf[5], buf[6]]);
    let mut reader = WireReader::new(&buf[7..]);

    let body = match msg_type {
        RLERROR => MessageBody::Rlerror(Rlerror::decode(&mut reader)?),
        TVERSION => MessageBody::Tversion(Tversion::decode(&mut reader)?),
        RVERSION => MessageBody::Rversion(Rversion::decode(&mut reader)?),
        TAUTH => MessageBody::Tauth(Tauth::decode(&mut reader)?),
        RAUTH => MessageBody::Rauth(Rauth::decode(&mut reader)?),
        TATTACH => MessageBody::Tattach(Tattach::decode(&mut reader)?),
        RATTACH => MessageBody::Rattach(Rattach::decode(&mut reader)?),
        TFLUSH => MessageBody::Tflush(Tflush::decode(&mut reader)?),
        RFLUSH => MessageBody::Rflush(Rflush::decode(&mut reader)?),
        TWALK => MessageBody::Twalk(Twalk::decode(&mut reader)?),
        RWALK => MessageBody::Rwalk(Rwalk::decode(&mut reader)?),
        TREAD => MessageBody::Tread(Tread::decode(&mut reader)?),
        RREAD => MessageBody::Rread(Rread::decode(&mut reader)?),
        TWRITE => MessageBody::Twrite(Twrite::decode(&mut reader)?),
        RWRITE => MessageBody::Rwrite(Rwrite::decode(&mut reader)?),
        TCLUNK => MessageBody::Tclunk(Tclunk::decode(&mut reader)?),
        RCLUNK => MessageBody::Rclunk(Rclunk::decode(&mut reader)?),
        TREMOVE => MessageBody::Tremove(Tremove::decode(&mut reader)?),
        RREMOVE => MessageBody::Rremove(Rremove::decode(&mut reader)?),
        TSTATFS => MessageBody::Tstatfs(Tstatfs::decode(&mut reader)?),
        RSTATFS => MessageBody::Rstatfs(Rstatfs::decode(&mut reader)?),
        TLOPEN => MessageBody::Tlopen(Tlopen::decode(&mut reader)?),
        RLOPEN => MessageBody::Rlopen(Rlopen::decode(&mut reader)?),
        TLCREATE => MessageBody::Tlcreate(Tlcreate::decode(&mut reader)?),
        RLCREATE => MessageBody::Rlcreate(Rlcreate::decode(&mut reader)?),
        TSYMLINK => MessageBody::Tsymlink(Tsymlink::decode(&mut reader)?),
        RSYMLINK => MessageBody::Rsymlink(Rsymlink::decode(&mut reader)?),
        TMKNOD => MessageBody::Tmknod(Tmknod::decode(&mut reader)?),
        RMKNOD => MessageBody::Rmknod(Rmknod::decode(&mut reader)?),
        TRENAME => MessageBody::Trename(Trename::decode(&mut reader)?),
        RRENAME => MessageBody::Rrename(Rrename::decode(&mut reader)?),
        TREADLINK => MessageBody::Treadlink(Treadlink::decode(&mut reader)?),
        RREADLINK => MessageBody::Rreadlink(Rreadlink::decode(&mut reader)?),
        TGETATTR => MessageBody::Tgetattr(Tgetattr::decode(&mut reader)?),
        RGETATTR => MessageBody::Rgetattr(Rgetattr::decode(&mut reader)?),
        TSETATTR => MessageBody::Tsetattr(Tsetattr::decode(&mut reader)?),
        RSETATTR => MessageBody::Rsetattr(Rsetattr::decode(&mut reader)?),
        TXATTRWALK => MessageBody::Txattrwalk(Txattrwalk::decode(&mut reader)?),
        RXATTRWALK => MessageBody::Rxattrwalk(Rxattrwalk::decode(&mut reader)?),
        TXATTRCREATE => MessageBody::Txattrcreate(Txattrcreate::decode(&mut reader)?),
        RXATTRCREATE => MessageBody::Rxattrcreate(Rxattrcreate::decode(&mut reader)?),
        TREADDIR => MessageBody::Treaddir(Treaddir::decode(&mut reader)?),
        RREADDIR => MessageBody::Rreaddir(Rreaddir::decode(&mut reader)?),
        TFSYNC => MessageBody::Tfsync(Tfsync::decode(&mut reader)?),
        RFSYNC => MessageBody::Rfsync(Rfsync::decode(&mut reader)?),
        TLOCK => MessageBody::Tlock(Tlock::decode(&mut reader)?),
        RLOCK => MessageBody::Rlock(Rlock::decode(&mut reader)?),
        TGETLOCK => MessageBody::Tgetlock(Tgetlock::decode(&mut reader)?),
        RGETLOCK => MessageBody::Rgetlock(Rgetlock::decode(&mut reader)?),
        TLINK => MessageBody::Tlink(Tlink::decode(&mut reader)?),
        RLINK => MessageBody::Rlink(Rlink::decode(&mut reader)?),
        TMKDIR => MessageBody::Tmkdir(Tmkdir::decode(&mut reader)?),
        RMKDIR => MessageBody::Rmkdir(Rmkdir::decode(&mut reader)?),
        TRENAMEAT => MessageBody::Trenameat(Trenameat::decode(&mut reader)?),
        RRENAMEAT => MessageBody::Rrenameat(Rrenameat::decode(&mut reader)?),
        TUNLINKAT => MessageBody::Tunlinkat(Tunlinkat::decode(&mut reader)?),
        RUNLINKAT => MessageBody::Runlinkat(Runlinkat::decode(&mut reader)?),
        _ => return Err(P9Error::UnknownMessageType { msg_type }),
    };

    Ok(ParsedMessage { tag, body })
}
