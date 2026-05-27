use std::collections::HashMap;
use wasm_bindgen::prelude::*;

use crate::compiler::{Compiler, CompilerOptions, FileResolver};
use crate::errors::Diagnostic;

#[wasm_bindgen]
pub struct WasmCompiler {
    lang_spec: Option<String>,
    files: HashMap<String, String>,
    last_diagnostics: Vec<Diagnostic>,
    last_error: String,
    require_entry_point: bool,
    collect_all_errors: bool,
}

#[wasm_bindgen]
impl WasmCompiler {
    #[wasm_bindgen(constructor)]
    pub fn new() -> WasmCompiler {
        WasmCompiler {
            lang_spec: None,
            files: HashMap::new(),
            last_diagnostics: Vec::new(),
            last_error: String::new(),
            require_entry_point: true,
            collect_all_errors: false,
        }
    }

    #[wasm_bindgen(js_name = "setLanguageSpec")]
    pub fn set_language_spec(&mut self, spec: &str) {
        self.lang_spec = Some(spec.to_string());
    }

    #[wasm_bindgen(js_name = "setRequireEntryPoint")]
    pub fn set_require_entry_point(&mut self, v: bool) {
        self.require_entry_point = v;
    }

    #[wasm_bindgen(js_name = "setCollectAllErrors")]
    pub fn set_collect_all_errors(&mut self, v: bool) {
        self.collect_all_errors = v;
    }

    #[wasm_bindgen(js_name = "addFile")]
    pub fn add_file(&mut self, name: &str, content: &str) {
        self.files.insert(name.to_string(), content.to_string());
    }

    #[wasm_bindgen(js_name = "removeFile")]
    pub fn remove_file(&mut self, name: &str) {
        self.files.remove(name);
    }

    #[wasm_bindgen(js_name = "clearFiles")]
    pub fn clear_files(&mut self) {
        self.files.clear();
    }

    pub fn compile(&mut self, filename: &str) -> i32 {
        let source = match self.files.get(filename) {
            Some(s) => s.clone(),
            None => {
                self.last_error = format!(
                    "{}.nss(0): ERROR: File not found: {}",
                    filename, filename
                );
                self.last_diagnostics.clear();
                return -1;
            }
        };

        let mut compiler = Compiler::new(CompilerOptions {
            require_entry_point: self.require_entry_point,
            collect_all_errors: self.collect_all_errors,
            ..Default::default()
        });

        if let Some(spec) = &self.lang_spec {
            compiler.set_language_spec(spec);
        }

        let resolver = WasmResolver { files: &self.files };
        let script_name = format!("{}.nss", filename);
        let result = compiler.compile(&source, &script_name, &resolver);

        self.last_diagnostics = result.diagnostics;

        if self.last_diagnostics.is_empty() {
            self.last_error = String::new();
            0
        } else {
            self.last_error = format!("{}", self.last_diagnostics[0]);
            self.last_diagnostics[0].error.strref()
        }
    }

    #[wasm_bindgen(js_name = "getLastError")]
    pub fn get_last_error(&self) -> String {
        self.last_error.clone()
    }

    #[wasm_bindgen(js_name = "getCollectedErrorCount")]
    pub fn get_collected_error_count(&self) -> i32 {
        self.last_diagnostics.len() as i32
    }

    #[wasm_bindgen(js_name = "getCollectedError")]
    pub fn get_collected_error(&self, index: i32) -> String {
        if let Some(d) = self.last_diagnostics.get(index as usize) {
            format!("{}", d)
        } else {
            String::new()
        }
    }

    #[wasm_bindgen(js_name = "getCollectedErrorCode")]
    pub fn get_collected_error_code(&self, index: i32) -> i32 {
        if let Some(d) = self.last_diagnostics.get(index as usize) {
            d.error.strref()
        } else {
            0
        }
    }

    #[wasm_bindgen(js_name = "getABIVersion")]
    pub fn get_abi_version(&self) -> i32 {
        3
    }
}

struct WasmResolver<'a> {
    files: &'a HashMap<String, String>,
}

impl FileResolver for WasmResolver<'_> {
    fn resolve(&self, filename: &str) -> Option<String> {
        self.files.get(filename).cloned()
    }
}
