#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::process::Command;

use maac::external::{
    DiscoveredExternalProcessor, ExternalDependencyBytes, ExternalPermissions,
    ExternalProcessorDescriptor,
};
use maac::external_host::{
    ExternalHost, ExternalLoadRequest, ExternalParameterOverride, ExternalProcessRequest,
};
use maac::external_native::{NativeDynamicLibraryAdapter, NATIVE_ABI_ID, NATIVE_ADAPTER_ID};
use serde_json::json;

fn compile_module(source: &str) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("module.rs");
    let library_path = dir
        .path()
        .join(format!("fixture{}", std::env::consts::DLL_SUFFIX));
    fs::write(&source_path, source).unwrap();
    let status = Command::new("rustc")
        .arg("--crate-type=cdylib")
        .arg("--edition=2021")
        .arg("-O")
        .arg(&source_path)
        .arg("-o")
        .arg(&library_path)
        .status()
        .unwrap();
    assert!(status.success());
    fs::read(library_path).unwrap()
}

fn descriptor() -> ExternalProcessorDescriptor {
    let source = format!(
        r#"{{
          "format":"maac.external-processor-descriptor",
          "wire_version":1,
          "id":"native.test/1",
          "version":"1.0.0",
          "abi":"{NATIVE_ABI_ID}",
          "ports":[
            {{"name":"in","direction":"input","kind":"audio","channels":1,
             "cardinality":"single","zero_default":true,"empty_event_stream":null}},
            {{"name":"out","direction":"output","kind":"audio","channels":1,
             "cardinality":null,"zero_default":null,"empty_event_stream":null}}
          ],
          "events":{{"native_notes":false,"expression_kinds":[],"hit_keys":[],"message_protocols":[]}},
          "config":[],
          "parameters":[
            {{"id":"gain","source_name":"gain","unit":"1","default":1,"minimum":0,"maximum":8,
             "range_policy":"error"}}
          ],
          "parameter_rates":[
            {{"parameter_id":"gain","rate":"sample","interpolation":["step"]}}
          ],
          "feedthrough":[
            {{"output_port":"out","input_ports":["in"],"parameters":["gain"]}}
          ],
          "latency_frames":0,
          "state":{{"serialization_version":"native.test.state/1","reset_semantics":"reset_frame_zero","save_restore":true}},
          "determinism":{{"mode":"declared_deterministic","dependencies":[]}},
          "permissions":{{"assets":false,"network":false,"devices":false,"process_execution":false,"shared_memory":false}}
        }}"#
    );
    ExternalProcessorDescriptor::from_json(source.as_bytes()).unwrap()
}

fn discovered(implementation: Vec<u8>) -> DiscoveredExternalProcessor {
    DiscoveredExternalProcessor {
        node: vec!["native".into()],
        processor_type: "native.test/1".into(),
        adapter_id: NATIVE_ADAPTER_ID.into(),
        descriptor_hash: "sha256:test".into(),
        descriptor: descriptor(),
        dependencies: ExternalDependencyBytes {
            implementation,
            descriptor: b"descriptor".to_vec(),
            adapter: b"adapter".to_vec(),
            state: Some(vec![3]),
            closure: BTreeMap::new(),
        },
    }
}

const MODULE: &str = r#"
use std::ffi::c_void;
use std::slice;

struct Instance {
    gain: f64,
}

#[no_mangle]
pub extern "C" fn maac_external_v1_create() -> *mut c_void {
    Box::into_raw(Box::new(Instance { gain: 1.0 })).cast()
}

#[no_mangle]
pub unsafe extern "C" fn maac_external_v1_destroy(instance: *mut c_void) {
    if !instance.is_null() {
        drop(Box::from_raw(instance.cast::<Instance>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn maac_external_v1_restore_state(
    instance: *mut c_void,
    bytes: *const u8,
    len: usize,
) -> i32 {
    if instance.is_null() || bytes.is_null() || len != 1 {
        return 1;
    }
    let state = slice::from_raw_parts(bytes, len);
    (*instance.cast::<Instance>()).gain = state[0] as f64;
    0
}

#[no_mangle]
pub unsafe extern "C" fn maac_external_v1_apply_config(
    instance: *mut c_void,
    json: *const u8,
    len: usize,
) -> i32 {
    if instance.is_null() || json.is_null() || len == 0 {
        return 1;
    }
    (*instance.cast::<Instance>()).gain = 4.0;
    0
}

#[no_mangle]
pub unsafe extern "C" fn maac_external_v1_set_parameter(
    instance: *mut c_void,
    id: *const u8,
    id_len: usize,
    value: *const u8,
    value_len: usize,
) -> i32 {
    if instance.is_null() || id.is_null() || value.is_null() {
        return 1;
    }
    let id = slice::from_raw_parts(id, id_len);
    if id != b"gain" {
        return 2;
    }
    let value = slice::from_raw_parts(value, value_len);
    let text = match std::str::from_utf8(value) {
        Ok(text) => text,
        Err(_) => return 1,
    };
    let gain: f64 = match text.parse() {
        Ok(value) => value,
        Err(_) => return 1,
    };
    (*instance.cast::<Instance>()).gain = gain;
    0
}

#[no_mangle]
pub unsafe extern "C" fn maac_external_v1_process_f64_planar(
    instance: *mut c_void,
    frames: usize,
    input_channels: usize,
    inputs: *const *const f64,
    output_channels: usize,
    outputs: *mut *mut f64,
) -> i32 {
    if instance.is_null() || input_channels != 1 || output_channels != 1 {
        return 6;
    }
    let input_ptr = *inputs;
    let output_ptr = *outputs;
    if input_ptr.is_null() || output_ptr.is_null() {
        return 6;
    }
    let input = slice::from_raw_parts(input_ptr, frames);
    let output = slice::from_raw_parts_mut(output_ptr, frames);
    let gain = (*instance.cast::<Instance>()).gain;
    for i in 0..frames {
        output[i] = input[i] * gain;
    }
    0
}
"#;

#[test]
fn exact_module_bytes_load_and_execute_published_native_abi() {
    let implementation = compile_module(MODULE);
    assert!(!implementation.is_empty());
    let processor = discovered(implementation);

    let mut host = ExternalHost::new(ExternalPermissions::default(), false);
    host.register(Box::new(NativeDynamicLibraryAdapter))
        .unwrap();

    let overrides = [ExternalParameterOverride {
        id: "gain".into(),
        value: json!(2),
    }];
    let config = json!({"t":"record","fields":{}});
    let mut instance = host
        .load(ExternalLoadRequest {
            processor: &processor,
            config: &config,
            parameter_overrides: &overrides,
        })
        .unwrap();

    let input = [0.25, -0.5, 1.0, -2.0];
    let mut output = [0.0; 4];
    let inputs: [&[f64]; 1] = [&input];
    let mut outputs: [&mut [f64]; 1] = [&mut output];
    instance
        .process(ExternalProcessRequest {
            frames: input.len(),
            audio_inputs: &inputs,
            audio_outputs: &mut outputs,
        })
        .unwrap();

    assert_eq!(output, [0.5, -1.0, 2.0, -4.0]);
}

#[test]
fn missing_required_native_symbol_is_capability_failure() {
    let incomplete = compile_module(
        r#"
        #[no_mangle]
        pub extern "C" fn maac_external_v1_create() -> *mut std::ffi::c_void {
            std::ptr::null_mut()
        }
        "#,
    );
    let processor = discovered(incomplete);
    let mut host = ExternalHost::new(ExternalPermissions::default(), false);
    host.register(Box::new(NativeDynamicLibraryAdapter))
        .unwrap();
    let config = json!({});
    let failure = host
        .load(ExternalLoadRequest {
            processor: &processor,
            config: &config,
            parameter_overrides: &[],
        })
        .err()
        .unwrap();
    assert_eq!(failure.code, "E_CAPABILITY");
}

#[test]
fn native_adapter_rejects_mismatched_channel_lengths_before_calling_module() {
    let processor = discovered(compile_module(MODULE));
    let mut host = ExternalHost::new(ExternalPermissions::default(), false);
    host.register(Box::new(NativeDynamicLibraryAdapter))
        .unwrap();
    let config = json!({});
    let mut instance = host
        .load(ExternalLoadRequest {
            processor: &processor,
            config: &config,
            parameter_overrides: &[],
        })
        .unwrap();

    let input = [1.0, 2.0];
    let mut output = [0.0; 1];
    let inputs: [&[f64]; 1] = [&input];
    let mut outputs: [&mut [f64]; 1] = [&mut output];
    let failure = instance
        .process(ExternalProcessRequest {
            frames: 2,
            audio_inputs: &inputs,
            audio_outputs: &mut outputs,
        })
        .unwrap_err();
    assert_eq!(failure.code, "E_RESOURCE_LIMIT");
}
