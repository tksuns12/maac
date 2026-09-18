//! Executable host boundary for explicitly registered external processor ABIs.
//!
//! MaaC/1 §17 requires an ABI to have its own published contract. This module
//! therefore does not infer executable formats from filenames or descriptor
//! strings. Callers register an adapter for an exact ABI/adapter pair; that
//! adapter receives the already hash-verified implementation bytes.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde_json::Value;

use crate::external::{
    DiscoveredExternalProcessor, ExternalError, ExternalHostCapabilities, ExternalPermissions,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ExternalParameterOverride {
    pub id: String,
    pub value: Value,
}

#[derive(Clone, Debug)]
pub struct ExternalLoadRequest<'a> {
    pub processor: &'a DiscoveredExternalProcessor,
    pub config: &'a Value,
    pub parameter_overrides: &'a [ExternalParameterOverride],
}

#[derive(Debug)]
pub struct ExternalProcessRequest<'a> {
    pub frames: usize,
    pub audio_inputs: &'a [&'a [f64]],
    pub audio_outputs: &'a mut [&'a mut [f64]],
}

pub trait ExternalProcessorInstance: Send {
    fn restore_state(&mut self, state: &[u8]) -> Result<(), ExternalError>;
    fn apply_config(&mut self, config: &Value) -> Result<(), ExternalError>;
    fn set_parameter(&mut self, id: &str, value: &Value) -> Result<(), ExternalError>;
    fn process(&mut self, request: ExternalProcessRequest<'_>) -> Result<(), ExternalError>;
}

pub trait ExternalAbiAdapter: Send + Sync {
    fn abi_id(&self) -> &str;
    fn adapter_id(&self) -> &str;

    fn instantiate(
        &self,
        processor: &DiscoveredExternalProcessor,
    ) -> Result<Box<dyn ExternalProcessorInstance>, ExternalError>;
}

#[derive(Default)]
pub struct ExternalHost {
    adapters: BTreeMap<(String, String), Box<dyn ExternalAbiAdapter>>,
    permissions: ExternalPermissions,
    allow_nondeterministic: bool,
}

impl fmt::Debug for ExternalHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExternalHost")
            .field(
                "registered_adapters",
                &self.adapters.keys().collect::<Vec<_>>(),
            )
            .field("permissions", &self.permissions)
            .field("allow_nondeterministic", &self.allow_nondeterministic)
            .finish()
    }
}

impl ExternalHost {
    pub fn new(permissions: ExternalPermissions, allow_nondeterministic: bool) -> Self {
        Self {
            adapters: BTreeMap::new(),
            permissions,
            allow_nondeterministic,
        }
    }

    pub fn register(&mut self, adapter: Box<dyn ExternalAbiAdapter>) -> Result<(), ExternalError> {
        let abi = adapter.abi_id();
        let adapter_id = adapter.adapter_id();
        if abi.is_empty() || adapter_id.is_empty() {
            return Err(error(
                "E_SCHEMA",
                "external ABI and adapter identifiers must be nonempty",
            ));
        }
        let key = (abi.to_owned(), adapter_id.to_owned());
        if self.adapters.contains_key(&key) {
            return Err(error(
                "E_SCHEMA",
                format!("duplicate external ABI adapter registration {abi} / {adapter_id}"),
            ));
        }
        self.adapters.insert(key, adapter);
        Ok(())
    }

    pub fn load(
        &self,
        request: ExternalLoadRequest<'_>,
    ) -> Result<Box<dyn ExternalProcessorInstance>, ExternalError> {
        let processor = request.processor;
        let key = (
            processor.descriptor.abi.clone(),
            processor.adapter_id.clone(),
        );
        let adapter = self.adapters.get(&key).ok_or_else(|| {
            error(
                "E_CAPABILITY",
                format!(
                    "no executable host adapter is registered for ABI {} / adapter {}",
                    key.0, key.1
                ),
            )
        })?;

        let declared_parameters: BTreeSet<&str> = processor
            .descriptor
            .parameters
            .iter()
            .map(|parameter| parameter.id.as_str())
            .collect();
        let mut overrides = BTreeSet::new();
        for parameter in request.parameter_overrides {
            if !declared_parameters.contains(parameter.id.as_str()) {
                return Err(error(
                    "E_REFERENCE",
                    format!(
                        "parameter override {} is absent from the external descriptor",
                        parameter.id
                    ),
                ));
            }
            if !overrides.insert(parameter.id.as_str()) {
                return Err(error(
                    "E_SCHEMA",
                    format!("duplicate parameter override {}", parameter.id),
                ));
            }
        }

        let capabilities = ExternalHostCapabilities {
            abi_ids: [key.0.clone()].into_iter().collect(),
            adapter_ids: [key.1.clone()].into_iter().collect(),
            permissions: self.permissions,
            allow_nondeterministic: self.allow_nondeterministic,
        };
        capabilities.authorize(processor)?;

        let mut instance = adapter.instantiate(processor)?;
        if let Some(state) = processor.dependencies.state.as_deref() {
            instance.restore_state(state)?;
        }
        instance.apply_config(request.config)?;
        for parameter in request.parameter_overrides {
            instance.set_parameter(&parameter.id, &parameter.value)?;
        }
        Ok(instance)
    }
}

fn error(code: &'static str, message: impl Into<String>) -> ExternalError {
    ExternalError {
        code,
        message: message.into(),
    }
}
