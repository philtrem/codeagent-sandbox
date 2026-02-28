#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(line) = std::str::from_utf8(data) else {
        return;
    };

    // Both parsers must return Ok or Err, never panic.
    let _ = codeagent_control::parse_vm_message(line);
    let _ = codeagent_control::parse_host_message(line);
});
