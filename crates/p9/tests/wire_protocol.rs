use codeagent_p9::messages::*;
use codeagent_p9::qid::{Qid, qid_type};
use codeagent_p9::wire::{MAX_MESSAGE_SIZE, validate_message_size};

// ---------------------------------------------------------------------------
// Helper: round-trip a message through encode → decode and verify equality.
// ---------------------------------------------------------------------------

fn round_trip<M: Message + std::fmt::Debug + PartialEq + Clone>(msg: &M, tag: u16) {
    let wire = msg.to_wire(tag);

    // Verify the size field
    let size = u32::from_le_bytes([wire[0], wire[1], wire[2], wire[3]]);
    assert_eq!(size as usize, wire.len(), "size field mismatch");

    // Verify the type byte
    assert_eq!(wire[4], M::MSG_TYPE, "type byte mismatch");

    // Verify the tag
    let decoded_tag = u16::from_le_bytes([wire[5], wire[6]]);
    assert_eq!(decoded_tag, tag, "tag mismatch");

    // Parse back through the full parse_message path
    let parsed = parse_message(&wire).expect("parse_message failed");
    assert_eq!(parsed.tag, tag);

    // Also decode just the body portion directly
    let mut reader = codeagent_p9::wire::WireReader::new(&wire[7..]);
    let decoded = M::decode(&mut reader).expect("decode failed");
    assert_eq!(&decoded, msg, "round-trip value mismatch");
}

// ===========================================================================
// P9-01: Round-trip tests for all message types
// ===========================================================================

#[test]
fn p9_01a_round_trip_tversion() {
    round_trip(
        &Tversion {
            msize: 4_194_304,
            version: "9P2000.L".to_string(),
        },
        NOTAG,
    );
}

#[test]
fn p9_01b_round_trip_rversion() {
    round_trip(
        &Rversion {
            msize: 4_194_304,
            version: "9P2000.L".to_string(),
        },
        NOTAG,
    );
}

#[test]
fn p9_01c_round_trip_tattach() {
    round_trip(
        &Tattach {
            fid: 0,
            afid: u32::MAX,
            uname: "user".to_string(),
            aname: "/".to_string(),
            n_uname: 1000,
        },
        1,
    );
}

#[test]
fn p9_01d_round_trip_rattach() {
    round_trip(
        &Rattach {
            qid: Qid::directory(0, 1),
        },
        1,
    );
}

#[test]
fn p9_01e_round_trip_tauth() {
    round_trip(
        &Tauth {
            afid: 10,
            uname: "root".to_string(),
            aname: "/mnt".to_string(),
            n_uname: 0,
        },
        2,
    );
}

#[test]
fn p9_01f_round_trip_rauth() {
    round_trip(
        &Rauth {
            aqid: Qid::file(0, 999),
        },
        2,
    );
}

#[test]
fn p9_01g_round_trip_twalk() {
    round_trip(
        &Twalk {
            fid: 0,
            newfid: 1,
            wnames: vec!["src".to_string(), "main.rs".to_string()],
        },
        3,
    );
}

#[test]
fn p9_01h_round_trip_twalk_empty() {
    round_trip(
        &Twalk {
            fid: 5,
            newfid: 6,
            wnames: vec![],
        },
        4,
    );
}

#[test]
fn p9_01i_round_trip_rwalk() {
    round_trip(
        &Rwalk {
            wqids: vec![Qid::directory(1, 100), Qid::file(2, 200)],
        },
        3,
    );
}

#[test]
fn p9_01j_round_trip_tgetattr() {
    round_trip(
        &Tgetattr {
            fid: 1,
            request_mask: P9_GETATTR_BASIC,
        },
        5,
    );
}

#[test]
fn p9_01k_round_trip_rgetattr() {
    round_trip(
        &Rgetattr {
            valid: P9_GETATTR_BASIC,
            qid: Qid::file(3, 42),
            mode: 0o100644,
            uid: 1000,
            gid: 1000,
            nlink: 1,
            rdev: 0,
            size: 4096,
            blksize: 4096,
            blocks: 8,
            atime_sec: 1700000000,
            atime_nsec: 0,
            mtime_sec: 1700000000,
            mtime_nsec: 500_000_000,
            ctime_sec: 1700000000,
            ctime_nsec: 0,
            btime_sec: 0,
            btime_nsec: 0,
            generation: 0,
            data_version: 0,
        },
        5,
    );
}

#[test]
fn p9_01l_round_trip_tread() {
    round_trip(
        &Tread {
            fid: 7,
            offset: 1024,
            count: 65536,
        },
        6,
    );
}

#[test]
fn p9_01m_round_trip_rread() {
    round_trip(
        &Rread {
            data: vec![0xDE, 0xAD, 0xBE, 0xEF],
        },
        6,
    );
}

#[test]
fn p9_01n_round_trip_twrite() {
    round_trip(
        &Twrite {
            fid: 8,
            offset: 0,
            data: b"hello world".to_vec(),
        },
        7,
    );
}

#[test]
fn p9_01o_round_trip_rwrite() {
    round_trip(&Rwrite { count: 11 }, 7);
}

#[test]
fn p9_01p_round_trip_treaddir() {
    round_trip(
        &Treaddir {
            fid: 10,
            offset: 0,
            count: 8192,
        },
        8,
    );
}

#[test]
fn p9_01q_round_trip_rreaddir() {
    round_trip(
        &Rreaddir {
            data: vec![1, 2, 3, 4, 5],
        },
        8,
    );
}

#[test]
fn p9_01r_round_trip_tlopen() {
    round_trip(
        &Tlopen {
            fid: 1,
            flags: 0o2, // O_RDWR
        },
        9,
    );
}

#[test]
fn p9_01s_round_trip_rlopen() {
    round_trip(
        &Rlopen {
            qid: Qid::file(4, 55),
            iounit: 4_194_280,
        },
        9,
    );
}

#[test]
fn p9_01t_round_trip_tlcreate() {
    round_trip(
        &Tlcreate {
            fid: 0,
            name: "new_file.txt".to_string(),
            flags: 0o102, // O_CREAT | O_RDWR
            mode: 0o644,
            gid: 1000,
        },
        10,
    );
}

#[test]
fn p9_01u_round_trip_rlcreate() {
    round_trip(
        &Rlcreate {
            qid: Qid::file(1, 77),
            iounit: 4_194_280,
        },
        10,
    );
}

#[test]
fn p9_01v_round_trip_tmkdir() {
    round_trip(
        &Tmkdir {
            dfid: 0,
            name: "subdir".to_string(),
            mode: 0o755,
            gid: 1000,
        },
        11,
    );
}

#[test]
fn p9_01w_round_trip_rmkdir() {
    round_trip(
        &Rmkdir {
            qid: Qid::directory(1, 88),
        },
        11,
    );
}

#[test]
fn p9_01x_round_trip_tunlinkat() {
    round_trip(
        &Tunlinkat {
            dirfid: 0,
            name: "old_file.txt".to_string(),
            flags: 0,
        },
        12,
    );
}

#[test]
fn p9_01y_round_trip_tunlinkat_rmdir() {
    round_trip(
        &Tunlinkat {
            dirfid: 0,
            name: "old_dir".to_string(),
            flags: AT_REMOVEDIR,
        },
        13,
    );
}

#[test]
fn p9_01z_round_trip_trenameat() {
    round_trip(
        &Trenameat {
            olddirfid: 0,
            oldname: "before.txt".to_string(),
            newdirfid: 0,
            newname: "after.txt".to_string(),
        },
        14,
    );
}

#[test]
fn p9_01aa_round_trip_tsetattr() {
    round_trip(
        &Tsetattr {
            fid: 1,
            valid: P9_SETATTR_MODE | P9_SETATTR_SIZE,
            mode: 0o755,
            uid: 0,
            gid: 0,
            size: 0,
            atime_sec: 0,
            atime_nsec: 0,
            mtime_sec: 0,
            mtime_nsec: 0,
        },
        15,
    );
}

#[test]
fn p9_01ab_round_trip_tsymlink() {
    round_trip(
        &Tsymlink {
            fid: 0,
            name: "link".to_string(),
            symtgt: "../target".to_string(),
            gid: 1000,
        },
        16,
    );
}

#[test]
fn p9_01ac_round_trip_rsymlink() {
    round_trip(
        &Rsymlink {
            qid: Qid::symlink(1, 99),
        },
        16,
    );
}

#[test]
fn p9_01ad_round_trip_treadlink() {
    round_trip(&Treadlink { fid: 5 }, 17);
}

#[test]
fn p9_01ae_round_trip_rreadlink() {
    round_trip(
        &Rreadlink {
            target: "../target".to_string(),
        },
        17,
    );
}

#[test]
fn p9_01af_round_trip_tlink() {
    round_trip(
        &Tlink {
            dfid: 0,
            fid: 5,
            name: "hardlink".to_string(),
        },
        18,
    );
}

#[test]
fn p9_01ag_round_trip_tmknod() {
    round_trip(
        &Tmknod {
            dfid: 0,
            name: "devnull".to_string(),
            mode: 0o20666,
            major: 1,
            minor: 3,
            gid: 0,
        },
        19,
    );
}

#[test]
fn p9_01ah_round_trip_tflush() {
    round_trip(&Tflush { oldtag: 42 }, 20);
}

#[test]
fn p9_01ai_round_trip_tclunk() {
    round_trip(&Tclunk { fid: 99 }, 21);
}

#[test]
fn p9_01aj_round_trip_tremove() {
    round_trip(&Tremove { fid: 50 }, 22);
}

#[test]
fn p9_01ak_round_trip_tstatfs() {
    round_trip(&Tstatfs { fid: 0 }, 23);
}

#[test]
fn p9_01al_round_trip_rstatfs() {
    round_trip(
        &Rstatfs {
            fs_type: 0x01021997, // V9FS_MAGIC
            bsize: 4096,
            blocks: 1_000_000,
            bfree: 500_000,
            bavail: 450_000,
            files: 100_000,
            ffree: 90_000,
            fsid: 0xABCD,
            namelen: 255,
        },
        23,
    );
}

#[test]
fn p9_01am_round_trip_tfsync() {
    round_trip(&Tfsync { fid: 7 }, 24);
}

#[test]
fn p9_01an_round_trip_rlerror() {
    round_trip(&Rlerror { ecode: 2 }, 25); // ENOENT
}

#[test]
fn p9_01ao_round_trip_tlock() {
    round_trip(
        &Tlock {
            fid: 1,
            lock_type: P9_LOCK_TYPE_WRLCK,
            flags: P9_LOCK_FLAGS_BLOCK,
            start: 0,
            length: 100,
            proc_id: 1234,
            client_id: "test".to_string(),
        },
        26,
    );
}

#[test]
fn p9_01ap_round_trip_rlock() {
    round_trip(&Rlock { status: P9_LOCK_SUCCESS }, 26);
}

#[test]
fn p9_01aq_round_trip_tgetlock() {
    round_trip(
        &Tgetlock {
            fid: 1,
            lock_type: P9_LOCK_TYPE_RDLCK,
            start: 0,
            length: 0,
            proc_id: 0,
            client_id: "".to_string(),
        },
        27,
    );
}

#[test]
fn p9_01ar_round_trip_rgetlock() {
    round_trip(
        &Rgetlock {
            lock_type: P9_LOCK_TYPE_UNLCK,
            start: 0,
            length: 0,
            proc_id: 0,
            client_id: "".to_string(),
        },
        27,
    );
}

#[test]
fn p9_01as_round_trip_txattrwalk() {
    round_trip(
        &Txattrwalk {
            fid: 1,
            newfid: 2,
            name: "user.test".to_string(),
        },
        28,
    );
}

#[test]
fn p9_01at_round_trip_rxattrwalk() {
    round_trip(&Rxattrwalk { size: 256 }, 28);
}

#[test]
fn p9_01au_round_trip_txattrcreate() {
    round_trip(
        &Txattrcreate {
            fid: 2,
            name: "user.test".to_string(),
            attr_size: 100,
            flags: 0,
        },
        29,
    );
}

#[test]
fn p9_01av_round_trip_trename() {
    round_trip(
        &Trename {
            fid: 5,
            dfid: 0,
            name: "newname.txt".to_string(),
        },
        30,
    );
}

#[test]
fn p9_01aw_round_trip_empty_body_messages() {
    // Test all R-messages with empty bodies
    round_trip(&Rclunk, 100);
    round_trip(&Rremove, 101);
    round_trip(&Rflush, 102);
    round_trip(&Rrename, 103);
    round_trip(&Rsetattr, 104);
    round_trip(&Rxattrcreate, 105);
    round_trip(&Rfsync, 106);
    round_trip(&Rlink, 107);
    round_trip(&Rrenameat, 108);
    round_trip(&Runlinkat, 109);
}

// ===========================================================================
// P9-02: Known-byte fixture tests
// ===========================================================================

/// P9-02a: Tversion with msize=8192, version="9P2000.L" must produce exact bytes.
#[test]
fn p9_02a_tversion_known_bytes() {
    let msg = Tversion {
        msize: 8192,
        version: "9P2000.L".to_string(),
    };
    let wire = msg.to_wire(NOTAG);

    // Total size: 4 (size) + 1 (type) + 2 (tag) + 4 (msize) + 2 (version len) + 8 (version) = 21
    assert_eq!(wire.len(), 21);

    // Size field (LE u32): 21 = 0x00000015
    assert_eq!(&wire[0..4], &[0x15, 0x00, 0x00, 0x00]);

    // Type byte: TVERSION = 100
    assert_eq!(wire[4], 100);

    // Tag: NOTAG = 0xFFFF
    assert_eq!(&wire[5..7], &[0xFF, 0xFF]);

    // msize: 8192 = 0x00002000
    assert_eq!(&wire[7..11], &[0x00, 0x20, 0x00, 0x00]);

    // Version string: length=8, "9P2000.L"
    assert_eq!(&wire[11..13], &[0x08, 0x00]); // u16 length
    assert_eq!(&wire[13..21], b"9P2000.L");
}

/// P9-02b: Rlerror with ecode=ENOENT (2) must produce exact bytes.
#[test]
fn p9_02b_rlerror_known_bytes() {
    let msg = Rlerror { ecode: 2 };
    let wire = msg.to_wire(42);

    // Total size: 4 + 1 + 2 + 4 = 11
    assert_eq!(wire.len(), 11);

    // Size: 11 = 0x0000000B
    assert_eq!(&wire[0..4], &[0x0B, 0x00, 0x00, 0x00]);

    // Type: RLERROR = 7
    assert_eq!(wire[4], 7);

    // Tag: 42 = 0x002A
    assert_eq!(&wire[5..7], &[0x2A, 0x00]);

    // ecode: 2 = 0x00000002
    assert_eq!(&wire[7..11], &[0x02, 0x00, 0x00, 0x00]);
}

/// P9-02c: Qid serialization matches the 13-byte layout.
#[test]
fn p9_02c_qid_known_bytes() {
    let qid = Qid::new(qid_type::QTDIR, 0x00000005, 0x0000000000000001);
    let mut writer = codeagent_p9::wire::WireWriter::new();
    writer.write_qid(&qid);
    let buf = writer.finish();

    // QID: 1 (type) + 4 (version) + 8 (path) = 13 bytes, plus 4 for size = 17
    assert_eq!(buf.len(), 17);

    // Skip size field, check QID bytes
    let qid_bytes = &buf[4..];
    assert_eq!(qid_bytes[0], qid_type::QTDIR); // type = 0x80
    assert_eq!(&qid_bytes[1..5], &[0x05, 0x00, 0x00, 0x00]); // version = 5
    assert_eq!(
        &qid_bytes[5..13],
        &[0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
    ); // path = 1
}

/// P9-02d: Tattach with specific values produces exact bytes.
#[test]
fn p9_02d_tattach_known_bytes() {
    let msg = Tattach {
        fid: 0,
        afid: 0xFFFFFFFF, // NOFID
        uname: "".to_string(),
        aname: "".to_string(),
        n_uname: 65534, // nobody
    };
    let wire = msg.to_wire(1);

    // 4 (size) + 1 (type) + 2 (tag) + 4 (fid) + 4 (afid) + 2+0 (uname) + 2+0 (aname) + 4 (n_uname) = 23
    assert_eq!(wire.len(), 23);

    // Type: TATTACH = 104
    assert_eq!(wire[4], TATTACH);

    // Tag: 1
    assert_eq!(&wire[5..7], &[0x01, 0x00]);

    // fid: 0
    assert_eq!(&wire[7..11], &[0x00, 0x00, 0x00, 0x00]);

    // afid: 0xFFFFFFFF
    assert_eq!(&wire[11..15], &[0xFF, 0xFF, 0xFF, 0xFF]);
}

/// P9-02e: Twalk with zero names (clone) produces exact bytes.
#[test]
fn p9_02e_twalk_clone_known_bytes() {
    let msg = Twalk {
        fid: 0,
        newfid: 1,
        wnames: vec![],
    };
    let wire = msg.to_wire(5);

    // 4 + 1 + 2 + 4 (fid) + 4 (newfid) + 2 (nwname=0) = 17
    assert_eq!(wire.len(), 17);
    assert_eq!(wire[4], TWALK);

    // nwname: 0
    assert_eq!(&wire[15..17], &[0x00, 0x00]);
}

// ===========================================================================
// P9-05: Oversized and malformed message rejection
// ===========================================================================

/// P9-05a: Message claiming 2GB size is rejected before allocation.
#[test]
fn p9_05a_giant_size_rejected() {
    let result = validate_message_size(2_000_000_000, MAX_MESSAGE_SIZE);
    assert!(result.is_err());
}

/// P9-05b: Message with size < 7 (minimum header) is rejected.
#[test]
fn p9_05b_undersize_rejected() {
    let result = validate_message_size(3, MAX_MESSAGE_SIZE);
    assert!(result.is_err());
}

/// P9-05c: String with length prefix exceeding remaining buffer is rejected.
#[test]
fn p9_05c_string_length_exceeds_buffer() {
    // Claim string is 1000 bytes but buffer only has 4 bytes total
    let buf = [0xE8, 0x03, 0x00, 0x00]; // u16 length = 1000
    let mut reader = codeagent_p9::wire::WireReader::new(&buf);
    assert!(reader.read_string().is_err());
}

/// P9-05d: parse_message rejects buffer shorter than 7 bytes.
#[test]
fn p9_05d_parse_too_short() {
    let buf = [0x05, 0x00, 0x00, 0x00, 0x64]; // 5-byte "message"
    let result = parse_message(&buf);
    assert!(result.is_err());
}

/// P9-05e: parse_message rejects size field mismatch with buffer length.
#[test]
fn p9_05e_size_mismatch() {
    // Size claims 100, but buffer is only 7 bytes
    let buf = [0x64, 0x00, 0x00, 0x00, TVERSION, 0xFF, 0xFF];
    let result = parse_message(&buf);
    assert!(result.is_err());
}

/// P9-05f: parse_message rejects unknown message type.
#[test]
fn p9_05f_unknown_message_type() {
    // Valid 7-byte envelope with unknown type 0xFF
    let buf = [0x07, 0x00, 0x00, 0x00, 0xFF, 0x00, 0x00];
    let result = parse_message(&buf);
    assert!(
        matches!(
            result.unwrap_err(),
            codeagent_p9::error::P9Error::UnknownMessageType { msg_type: 0xFF }
        )
    );
}

/// P9-05g: Truncated Tversion body (missing version string).
#[test]
fn p9_05g_truncated_tversion() {
    // Size=11, type=TVERSION, tag=0xFFFF, msize=8192 — but missing version string
    let buf = [
        0x0B, 0x00, 0x00, 0x00, // size = 11
        TVERSION, 0xFF, 0xFF, // type + tag
        0x00, 0x20, 0x00, 0x00, // msize = 8192
    ];
    let result = parse_message(&buf);
    assert!(result.is_err());
}
