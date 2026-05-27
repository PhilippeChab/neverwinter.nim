use std::collections::HashMap;
use wasm_bindgen::prelude::*;

use crate::ast::{AstArena, NodeId, NULL_NODE};
use crate::astquery::{self, PositionQuery};
use crate::compiler::{Compiler, CompilerOptions, FileResolver};
use crate::errors::Diagnostic;
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::semcheck::{FunctionSig, SemanticChecker, StructDef};

struct ParsedState {
    arena: AstArena,
    root: NodeId,
    file_names: Vec<String>,
    functions: Vec<FunctionSig>,
    structs: Vec<StructDef>,
}

#[wasm_bindgen]
pub struct WasmCompiler {
    lang_spec: Option<String>,
    files: HashMap<String, String>,
    last_diagnostics: Vec<Diagnostic>,
    last_error: String,
    last_ncs: Option<Vec<u8>>,
    last_parse: Option<ParsedState>,
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
            last_ncs: None,
            last_parse: None,
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
                self.last_parse = None;
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
        self.last_ncs = if result.success && !result.ncs.is_empty() {
            Some(result.ncs)
        } else {
            None
        };

        // Parse again to keep the AST for queries (the compiler consumes it)
        self.last_parse = self.parse_for_queries(&source, &script_name);

        if self.last_diagnostics.is_empty() {
            self.last_error = String::new();
            0
        } else {
            self.last_error = format!("{}", self.last_diagnostics[0]);
            self.last_diagnostics[0].error.strref()
        }
    }

    fn parse_for_queries(&self, source: &str, filename: &str) -> Option<ParsedState> {
        let mut lexer = Lexer::new(source, filename, 0);
        let tokens = lexer.tokenize();
        let mut parser = Parser::new(tokens);
        parser.file_names.push(filename.to_string());
        parser.set_collect_all_errors(true);
        parser.set_require_entry_point(false);
        let root = parser.parse_program().ok()?;
        if root == NULL_NODE { return None; }

        let arena = std::mem::replace(&mut parser.arena, AstArena::new());
        let file_names = parser.file_names;

        let mut checker = SemanticChecker::new(&arena, &file_names);
        checker.set_collect_all_errors(true);
        checker.set_require_entry_point(false);
        if let Some(spec) = &self.lang_spec {
            checker.load_lang_spec(spec);
        }
        let _ = checker.check(root);

        // SAFETY: checker borrows arena/file_names but we only need the
        // functions/structs vecs which are owned. We clone them before
        // the checker is dropped.
        let functions = checker.functions.clone();
        let structs = checker.structs.clone();

        Some(ParsedState { arena, root, file_names, functions, structs })
    }

    // ===== Error API =====

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

    // ===== NCS API =====

    #[wasm_bindgen(js_name = "getNcsBytes")]
    pub fn get_ncs_bytes(&self) -> Option<Vec<u8>> {
        self.last_ncs.clone()
    }

    #[wasm_bindgen(js_name = "getNcsSize")]
    pub fn get_ncs_size(&self) -> i32 {
        self.last_ncs.as_ref().map(|n| n.len() as i32).unwrap_or(0)
    }

    // ===== AST API =====

    #[wasm_bindgen(js_name = "getParseTreeJSON")]
    pub fn get_parse_tree_json(&self) -> String {
        match &self.last_parse {
            Some(state) => {
                let json = astquery::ast_to_json(&state.arena, state.root);
                serde_json::to_string(&json).unwrap_or_default()
            }
            None => "{}".to_string(),
        }
    }

    #[wasm_bindgen(js_name = "findNodeAtPosition")]
    pub fn find_node_at_position(&self, line: u32, col: u32) -> String {
        match &self.last_parse {
            Some(state) => {
                let q = PositionQuery::new(
                    &state.arena, state.root, &state.functions, &state.structs, &state.file_names,
                );
                match q.find_node_at_position(line, col) {
                    Some(node) => serde_json::to_string(&node).unwrap_or_default(),
                    None => "null".to_string(),
                }
            }
            None => "null".to_string(),
        }
    }

    #[wasm_bindgen(js_name = "getDefinitionAtPosition")]
    pub fn get_definition_at_position(&self, line: u32, col: u32) -> String {
        match &self.last_parse {
            Some(state) => {
                let q = PositionQuery::new(
                    &state.arena, state.root, &state.functions, &state.structs, &state.file_names,
                );
                match q.get_definition_at_position(line, col) {
                    Some(def) => serde_json::to_string(&def).unwrap_or_default(),
                    None => "null".to_string(),
                }
            }
            None => "null".to_string(),
        }
    }

    #[wasm_bindgen(js_name = "isInFunctionCall")]
    pub fn is_in_function_call(&self, line: u32, col: u32) -> bool {
        match &self.last_parse {
            Some(state) => {
                let q = PositionQuery::new(
                    &state.arena, state.root, &state.functions, &state.structs, &state.file_names,
                );
                q.is_in_function_call(line, col)
            }
            None => false,
        }
    }

    #[wasm_bindgen(js_name = "getFunctionNameAtPosition")]
    pub fn get_function_name_at_position(&self, line: u32, col: u32) -> String {
        match &self.last_parse {
            Some(state) => {
                let q = PositionQuery::new(
                    &state.arena, state.root, &state.functions, &state.structs, &state.file_names,
                );
                q.get_function_name_at_position(line, col).unwrap_or_default()
            }
            None => String::new(),
        }
    }

    #[wasm_bindgen(js_name = "getActiveParameterIndex")]
    pub fn get_active_parameter_index(&self, line: u32, col: u32) -> u32 {
        match &self.last_parse {
            Some(state) => {
                let q = PositionQuery::new(
                    &state.arena, state.root, &state.functions, &state.structs, &state.file_names,
                );
                q.get_active_parameter_index(line, col)
            }
            None => 0,
        }
    }

    #[wasm_bindgen(js_name = "getCompletionsAtPosition")]
    pub fn get_completions_at_position(&self, line: u32, col: u32) -> String {
        match &self.last_parse {
            Some(state) => {
                let q = PositionQuery::new(
                    &state.arena, state.root, &state.functions, &state.structs, &state.file_names,
                );
                let comps = q.get_completions_at_position(line, col);
                serde_json::to_string(&comps).unwrap_or_default()
            }
            None => "[]".to_string(),
        }
    }

    #[wasm_bindgen(js_name = "getABIVersion")]
    pub fn get_abi_version(&self) -> i32 {
        4
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
