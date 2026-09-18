//! Native dynamic-library adapter for the published MaaC external ABI v1.
//!
//! This adapter is selected only by the exact ABI and adapter identifiers. It
//! never guesses an ABI from a filename or library contents.

#[cfg(unix)]
mod imp {
    use std::ffi::{c_void, CString};
    use std::io::Write;
    use std::mem;
    use std::os::unix::ffi::OsStrExt;
    use std::ptr;

    use serde_json::Value;
    use tempfile::NamedTempFile;

    use crate::external::{DiscoveredExternalProcessor, ExternalError};
    use crate::external_host::{
        ExternalAbiAdapter, ExternalProcessRequest, ExternalProcessorInstance,
    };
    use crate::production_identity::canonical_json_bytes;

    pub const NATIVE_ABI_ID: &str = "maac.native-c-abi/1";
    pub const NATIVE_ADAPTER_ID: &str = "maac.native-dylib-adapter/1";

    type CreateFn = unsafe extern "C" fn() -> *mut c_void;
    type DestroyFn = unsafe extern "C" fn(*mut c_void);
    type RestoreStateFn = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> i32;
    type ApplyConfigFn = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> i32;
    type SetParameterFn =
        unsafe extern "C" fn(*mut c_void, *const u8, usize, *const u8, usize) -> i32;
    type ProcessFn = unsafe extern "C" fn(
        *mut c_void,
        usize,
        usize,
        *const *const f64,
        usize,
        *mut *mut f64,
    ) -> i32;

    struct NativeSymbols {
        create: CreateFn,
        destroy: DestroyFn,
        restore_state: RestoreStateFn,
        apply_config: ApplyConfigFn,
        set_parameter: SetParameterFn,
        process: ProcessFn,
    }

    pub struct NativeDynamicLibraryAdapter;

    impl ExternalAbiAdapter for NativeDynamicLibraryAdapter {
        fn abi_id(&self) -> &str {
            NATIVE_ABI_ID
        }

        fn adapter_id(&self) -> &str {
            NATIVE_ADAPTER_ID
        }

        fn instantiate(
            &self,
            processor: &DiscoveredExternalProcessor,
        ) -> Result<Box<dyn ExternalProcessorInstance>, ExternalError> {
            let mut file = tempfile::Builder::new()
                .prefix("maac-external-")
                .suffix(std::env::consts::DLL_SUFFIX)
                .tempfile()
                .map_err(|e| error("E_RESOURCE_LIMIT", format!("temporary module: {e}")))?;
            file.write_all(&processor.dependencies.implementation)
                .map_err(|e| error("E_RESOURCE_LIMIT", format!("write module bytes: {e}")))?;
            file.flush()
                .map_err(|e| error("E_RESOURCE_LIMIT", format!("flush module bytes: {e}")))?;

            let path = CString::new(file.path().as_os_str().as_bytes())
                .map_err(|_| error("E_SCHEMA", "temporary module path contains NUL"))?;
            let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
            if handle.is_null() {
                return Err(error(
                    "E_CAPABILITY",
                    format!("dynamic loader rejected external module: {}", dl_error()),
                ));
            }

            let symbols = unsafe {
                match load_symbols(handle) {
                    Ok(symbols) => symbols,
                    Err(failure) => {
                        libc::dlclose(handle);
                        return Err(failure);
                    }
                }
            };

            let instance = unsafe { (symbols.create)() };
            if instance.is_null() {
                unsafe {
                    libc::dlclose(handle);
                }
                return Err(error(
                    "E_CAPABILITY",
                    "external module create returned a null instance",
                ));
            }

            Ok(Box::new(NativeDynamicLibraryInstance {
                instance,
                symbols,
                handle,
                _module_file: file,
            }))
        }
    }

    struct NativeDynamicLibraryInstance {
        instance: *mut c_void,
        symbols: NativeSymbols,
        handle: *mut c_void,
        _module_file: NamedTempFile,
    }

    unsafe impl Send for NativeDynamicLibraryInstance {}

    impl Drop for NativeDynamicLibraryInstance {
        fn drop(&mut self) {
            unsafe {
                (self.symbols.destroy)(self.instance);
                libc::dlclose(self.handle);
            }
        }
    }

    impl ExternalProcessorInstance for NativeDynamicLibraryInstance {
        fn restore_state(&mut self, state: &[u8]) -> Result<(), ExternalError> {
            let status =
                unsafe { (self.symbols.restore_state)(self.instance, state.as_ptr(), state.len()) };
            status_result(status, "restore_state")
        }

        fn apply_config(&mut self, config: &Value) -> Result<(), ExternalError> {
            let bytes = canonical(config)?;
            let status =
                unsafe { (self.symbols.apply_config)(self.instance, bytes.as_ptr(), bytes.len()) };
            status_result(status, "apply_config")
        }

        fn set_parameter(&mut self, id: &str, value: &Value) -> Result<(), ExternalError> {
            let bytes = canonical(value)?;
            let status = unsafe {
                (self.symbols.set_parameter)(
                    self.instance,
                    id.as_bytes().as_ptr(),
                    id.len(),
                    bytes.as_ptr(),
                    bytes.len(),
                )
            };
            status_result(status, "set_parameter")
        }

        fn process(&mut self, request: ExternalProcessRequest<'_>) -> Result<(), ExternalError> {
            for (index, input) in request.audio_inputs.iter().enumerate() {
                if input.len() != request.frames {
                    return Err(error(
                        "E_RESOURCE_LIMIT",
                        format!("input channel {index} length differs from frame count"),
                    ));
                }
            }
            for (index, output) in request.audio_outputs.iter().enumerate() {
                if output.len() != request.frames {
                    return Err(error(
                        "E_RESOURCE_LIMIT",
                        format!("output channel {index} length differs from frame count"),
                    ));
                }
            }

            let inputs: Vec<*const f64> = request
                .audio_inputs
                .iter()
                .map(|channel| channel.as_ptr())
                .collect();
            let mut outputs: Vec<*mut f64> = request
                .audio_outputs
                .iter_mut()
                .map(|channel| channel.as_mut_ptr())
                .collect();

            let input_ptr = if inputs.is_empty() {
                ptr::null()
            } else {
                inputs.as_ptr()
            };
            let output_ptr = if outputs.is_empty() {
                ptr::null_mut()
            } else {
                outputs.as_mut_ptr()
            };
            let status = unsafe {
                (self.symbols.process)(
                    self.instance,
                    request.frames,
                    inputs.len(),
                    input_ptr,
                    outputs.len(),
                    output_ptr,
                )
            };
            status_result(status, "process")
        }
    }

    fn canonical(value: &Value) -> Result<Vec<u8>, ExternalError> {
        canonical_json_bytes(value).map_err(|e| error("E_SCHEMA", e.to_string()))
    }

    fn status_result(status: i32, operation: &str) -> Result<(), ExternalError> {
        if status == 0 {
            return Ok(());
        }
        let code = match status {
            1 => "E_SCHEMA",
            2 => "E_REFERENCE",
            3 => "E_CAPABILITY",
            4 => "E_PERMISSION",
            5 => "E_RESOURCE_LIMIT",
            6 => "E_RENDER_STATE",
            _ => "E_RENDER_STATE",
        };
        Err(error(
            code,
            format!("native external ABI operation {operation} failed with status {status}"),
        ))
    }

    unsafe fn load_symbols(handle: *mut c_void) -> Result<NativeSymbols, ExternalError> {
        macro_rules! symbol {
            ($name:literal, $ty:ty) => {{
                let raw = load_symbol(handle, concat!($name, "\0").as_bytes())?;
                mem::transmute::<*mut c_void, $ty>(raw)
            }};
        }
        Ok(NativeSymbols {
            create: symbol!("maac_external_v1_create", CreateFn),
            destroy: symbol!("maac_external_v1_destroy", DestroyFn),
            restore_state: symbol!("maac_external_v1_restore_state", RestoreStateFn),
            apply_config: symbol!("maac_external_v1_apply_config", ApplyConfigFn),
            set_parameter: symbol!("maac_external_v1_set_parameter", SetParameterFn),
            process: symbol!("maac_external_v1_process_f64_planar", ProcessFn),
        })
    }

    unsafe fn load_symbol(
        handle: *mut c_void,
        nul_terminated_name: &[u8],
    ) -> Result<*mut c_void, ExternalError> {
        libc::dlerror();
        let raw = libc::dlsym(handle, nul_terminated_name.as_ptr().cast());
        let failure = libc::dlerror();
        if !failure.is_null() || raw.is_null() {
            return Err(error(
                "E_CAPABILITY",
                format!(
                    "external module is missing required symbol {}",
                    String::from_utf8_lossy(
                        &nul_terminated_name[..nul_terminated_name.len().saturating_sub(1)]
                    )
                ),
            ));
        }
        Ok(raw)
    }

    fn dl_error() -> String {
        unsafe {
            let message = libc::dlerror();
            if message.is_null() {
                return "unknown loader error".into();
            }
            std::ffi::CStr::from_ptr(message)
                .to_string_lossy()
                .into_owned()
        }
    }

    fn error(code: &'static str, message: impl Into<String>) -> ExternalError {
        ExternalError {
            code,
            message: message.into(),
        }
    }
}

#[cfg(unix)]
pub use imp::{NativeDynamicLibraryAdapter, NATIVE_ABI_ID, NATIVE_ADAPTER_ID};

#[cfg(not(unix))]
pub const NATIVE_ABI_ID: &str = "maac.native-c-abi/1";
#[cfg(not(unix))]
pub const NATIVE_ADAPTER_ID: &str = "maac.native-dylib-adapter/1";
