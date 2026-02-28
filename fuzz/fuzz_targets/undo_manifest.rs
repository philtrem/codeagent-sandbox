#![no_main]

use libfuzzer_sys::fuzz_target;

use codeagent_interceptor::manifest::StepManifest;

fuzz_target!(|data: &[u8]| {
    let Ok(json) = std::str::from_utf8(data) else {
        return;
    };

    let _ = serde_json::from_str::<StepManifest>(json);
});
