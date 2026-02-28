#![no_main]

use std::path::Path;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(path) = std::str::from_utf8(data) else {
        return;
    };

    let root = Path::new("/sandbox/working");

    // Both validators use the same algorithm but return different error types.
    // Neither should ever panic.
    let _ = codeagent_mcp::validate_path(path, root);
    let _ = codeagent_stdio::validate_path(path, root);
});
