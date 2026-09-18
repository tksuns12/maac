use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use maac::external::{
    DiscoveredExternalProcessor, ExternalDependencyBytes, ExternalPermissions,
    ExternalProcessorDescriptor,
};
use maac::external_host::{
    ExternalAbiAdapter, ExternalHost, ExternalLoadRequest, ExternalParameterOverride,
    ExternalProcessRequest, ExternalProcessorInstance,
};
use serde_json::{json, Value};

fn descriptor() -> ExternalProcessorDescriptor {
    ExternalProcessorDescriptor::from_json(
        br#"{
          "format":"maac.external-processor-descriptor",
          "wire_version":1,
          "id":"test.external/1",
          "version":"1.0.0",
          "abi":"test.abi/1",
          "ports":[
            {"name":"in","direction":"input","kind":"audio","channels":1,
             "cardinality":"single","zero_default":true,"empty_event_stream":null},
            {"name":"out","direction":"output","kind":"audio","channels":1,
             "cardinality":null,"zero_default":null,"empty_event_stream":null}
          ],
          "events":{"native_notes":false,"expression_kinds":[],"hit_keys":[],"message_protocols":[]},
          "config":[],
          "parameters":[
            {"id":"gain","source_name":"gain","unit":"1","default":1,"minimum":0,"maximum":4,
             "range_policy":"error"}
          ],
          "parameter_rates":[
            {"parameter_id":"gain","rate":"sample","interpolation":["step"]}
          ],
          "feedthrough":[
            {"output_port":"out","input_ports":["in"],"parameters":["gain"]}
          ],
          "latency_frames":0,
          "state":{"serialization_version":"test.state/1","reset_semantics":"reset_frame_zero","save_restore":true},
          "determinism":{"mode":"declared_deterministic","dependencies":[]},
          "permissions":{"assets":true,"network":false,"devices":false,"process_execution":false,"shared_memory":false}
        }"#,
    )
    .unwrap()
}

fn discovered() -> DiscoveredExternalProcessor {
    DiscoveredExternalProcessor {
        node: vec!["fx".into()],
        processor_type: "test.external/1".into(),
        adapter_id: "test.adapter/1".into(),
        descriptor_hash: "sha256:test".into(),
        descriptor: descriptor(),
        dependencies: ExternalDependencyBytes {
            implementation: b"impl/1".to_vec(),
            descriptor: b"descriptor".to_vec(),
            adapter: b"adapter/1".to_vec(),
            state: Some(vec![1, 2, 3, 4]),
            closure: BTreeMap::new(),
        },
    }
}

struct TestAdapter {
    log: Arc<Mutex<Vec<String>>>,
}

impl ExternalAbiAdapter for TestAdapter {
    fn abi_id(&self) -> &str {
        "test.abi/1"
    }

    fn adapter_id(&self) -> &str {
        "test.adapter/1"
    }

    fn instantiate(
        &self,
        processor: &DiscoveredExternalProcessor,
    ) -> Result<Box<dyn ExternalProcessorInstance>, maac::external::ExternalError> {
        assert_eq!(processor.dependencies.implementation, b"impl/1");
        assert_eq!(processor.dependencies.adapter, b"adapter/1");
        self.log.lock().unwrap().push("instantiate".into());
        Ok(Box::new(TestInstance {
            log: self.log.clone(),
            gain: 1.0,
        }))
    }
}

struct TestInstance {
    log: Arc<Mutex<Vec<String>>>,
    gain: f64,
}

impl ExternalProcessorInstance for TestInstance {
    fn restore_state(&mut self, state: &[u8]) -> Result<(), maac::external::ExternalError> {
        self.log.lock().unwrap().push(format!("state:{state:?}"));
        Ok(())
    }

    fn apply_config(&mut self, config: &Value) -> Result<(), maac::external::ExternalError> {
        self.log
            .lock()
            .unwrap()
            .push(format!("config:{}", config["name"]));
        Ok(())
    }

    fn set_parameter(
        &mut self,
        id: &str,
        value: &Value,
    ) -> Result<(), maac::external::ExternalError> {
        self.log
            .lock()
            .unwrap()
            .push(format!("parameter:{id}={value}"));
        if id == "gain" {
            self.gain = value.as_i64().unwrap() as f64;
        }
        Ok(())
    }

    fn process(
        &mut self,
        request: ExternalProcessRequest<'_>,
    ) -> Result<(), maac::external::ExternalError> {
        assert_eq!(request.audio_inputs.len(), 1);
        assert_eq!(request.audio_outputs.len(), 1);
        assert_eq!(request.audio_inputs[0].len(), request.frames);
        assert_eq!(request.audio_outputs[0].len(), request.frames);
        for frame in 0..request.frames {
            request.audio_outputs[0][frame] = request.audio_inputs[0][frame] * self.gain;
        }
        self.log.lock().unwrap().push("process".into());
        Ok(())
    }
}

#[test]
fn exact_registration_authorization_and_load_order_precede_processing() {
    let processor = discovered();
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut host = ExternalHost::new(
        ExternalPermissions {
            assets: true,
            ..ExternalPermissions::default()
        },
        false,
    );
    host.register(Box::new(TestAdapter { log: log.clone() }))
        .unwrap();

    let overrides = [ExternalParameterOverride {
        id: "gain".into(),
        value: json!(2),
    }];
    let config = json!({"name":"locked"});
    let mut instance = host
        .load(ExternalLoadRequest {
            processor: &processor,
            config: &config,
            parameter_overrides: &overrides,
        })
        .unwrap();

    assert_eq!(
        *log.lock().unwrap(),
        vec![
            "instantiate",
            "state:[1, 2, 3, 4]",
            "config:\"locked\"",
            "parameter:gain=2"
        ]
    );

    let input = [0.25, -0.5, 1.0];
    let mut output = [0.0; 3];
    let inputs: [&[f64]; 1] = [&input];
    let mut outputs: [&mut [f64]; 1] = [&mut output];
    instance
        .process(ExternalProcessRequest {
            frames: 3,
            audio_inputs: &inputs,
            audio_outputs: &mut outputs,
        })
        .unwrap();
    assert_eq!(output, [0.5, -1.0, 2.0]);
}

#[test]
fn unregistered_or_permission_denied_processors_never_instantiate() {
    let processor = discovered();
    let config = json!({});
    let empty = [];

    let host = ExternalHost::new(
        ExternalPermissions {
            assets: true,
            ..ExternalPermissions::default()
        },
        false,
    );
    assert_eq!(
        host.load(ExternalLoadRequest {
            processor: &processor,
            config: &config,
            parameter_overrides: &empty,
        })
        .err()
        .unwrap()
        .code,
        "E_CAPABILITY"
    );

    let log = Arc::new(Mutex::new(Vec::new()));
    let mut denied = ExternalHost::new(ExternalPermissions::default(), false);
    denied
        .register(Box::new(TestAdapter { log: log.clone() }))
        .unwrap();
    assert_eq!(
        denied
            .load(ExternalLoadRequest {
                processor: &processor,
                config: &config,
                parameter_overrides: &empty,
            })
            .err()
            .unwrap()
            .code,
        "E_PERMISSION"
    );
    assert!(log.lock().unwrap().is_empty());
}

#[test]
fn unknown_parameter_override_is_rejected_before_instantiation() {
    let processor = discovered();
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut host = ExternalHost::new(
        ExternalPermissions {
            assets: true,
            ..ExternalPermissions::default()
        },
        false,
    );
    host.register(Box::new(TestAdapter { log: log.clone() }))
        .unwrap();
    let overrides = [ExternalParameterOverride {
        id: "missing".into(),
        value: json!(1),
    }];
    let config = json!({"name":"locked"});
    let error = host
        .load(ExternalLoadRequest {
            processor: &processor,
            config: &config,
            parameter_overrides: &overrides,
        })
        .err()
        .unwrap();
    assert_eq!(error.code, "E_REFERENCE");
    assert!(log.lock().unwrap().is_empty());
}
