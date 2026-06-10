#![no_main]
//! Fuzz the protocol wire decoders and the hand-written validators behind the
//! validated newtypes. The contract under test is simply: decoding hostile
//! input must never panic, hang, or abort — only return `Ok`/`Err`.

use libfuzzer_sys::fuzz_target;

use seedling_protocol::{
    actor::Actor,
    env::{EnvVar, EnvironmentVarName},
    names::{ActionName, AppName, ParamName},
};

fuzz_target!(|data: &[u8]| {
    // Raw bytes at the structured wire types (serde_json + container-level
    // Deserialize impls).
    let _ = serde_json::from_slice::<Actor>(data);
    let _ = serde_json::from_slice::<EnvVar>(data);

    // Arbitrary text wrapped as a JSON string and fed to the validated
    // newtypes, so their custom Deserialize validators (validate_bsl_name,
    // validate_env_name) see hostile input even when the raw bytes are not
    // themselves valid JSON.
    let text = String::from_utf8_lossy(data);
    if let Ok(json) = serde_json::to_string(&text) {
        let _ = serde_json::from_str::<AppName>(&json);
        let _ = serde_json::from_str::<ActionName>(&json);
        let _ = serde_json::from_str::<ParamName>(&json);
        let _ = serde_json::from_str::<EnvironmentVarName>(&json);
    }
});
