use crate::ast::{AstArena, AstNode, NodeId, NULL_NODE, Operation};
use crate::errors::{CompileError, Diagnostic};
use crate::types::NwType;

fn preprocess_lang_spec(spec: &str) -> String {
    let mut result = Vec::new();
    let mut engine_structs: Vec<(u8, String)> = Vec::new();

    for line in spec.lines() {
        let trimmed = line.trim();

        // Parse #define ENGINE_STRUCTURE_N name
        if trimmed.starts_with("#define ENGINE_STRUCTURE_") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                let define_name = parts[1];
                let struct_name = parts[2];
                if let Some(idx_str) = define_name.strip_prefix("ENGINE_STRUCTURE_") {
                    if let Ok(idx) = idx_str.parse::<u8>() {
                        engine_structs.push((idx, struct_name.to_string()));
                    }
                }
            }
            continue;
        }

        // Skip other #define directives
        if trimmed.starts_with("#define") {
            continue;
        }

        // Skip empty lines and comments
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }

        result.push(line);
    }

    // Register engine structure keywords
    if !engine_structs.is_empty() {
        let mappings: Vec<(u8, &str)> = engine_structs.iter().map(|(i, n)| (*i, n.as_str())).collect();
        crate::lexer::set_engine_structures(&mappings);
    }

    result.join("\n")
}

#[derive(Debug, Clone)]
pub struct FunctionSig {
    pub name: String,
    pub return_type: NwType,
    pub return_type_name: Option<String>,
    pub params: Vec<ParamInfo>,
    pub has_implementation: bool,
    pub is_engine_action: bool,
    pub action_id: u32,
}

#[derive(Debug, Clone)]
pub enum DefaultValue {
    Integer(i32),
    Float(f32),
    String(String),
    Object(i32),
    Vector(f32, f32, f32),
    /// For engine-struct types (location, json) C++ stores raw payload; we keep
    /// a placeholder and emit the engine-specific constant at call time.
    EngineStruct,
}

#[derive(Debug, Clone)]
pub struct ParamInfo {
    pub name: String,
    pub nw_type: NwType,
    pub type_name: Option<String>,
    pub has_default: bool,
    pub default_value: Option<DefaultValue>,
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<FieldInfo>,
    pub byte_size: i32,
}

#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub name: String,
    pub nw_type: NwType,
    pub type_name: Option<String>,
    pub offset: i32,
}

#[derive(Debug, Clone)]
struct VarEntry {
    name: String,
    nw_type: NwType,
    type_name: Option<String>,
    scope_level: u32,
    is_constant: bool,
    /// Captured initializer if it's a constant expression — used to resolve
    /// engine-constant references in parameter default values.
    const_value: Option<DefaultValue>,
}

pub struct SemanticChecker<'a> {
    arena: &'a AstArena,
    file_names: &'a [String],
    pub diagnostics: Vec<Diagnostic>,
    pub functions: Vec<FunctionSig>,
    pub structs: Vec<StructDef>,
    pub globals: Vec<VarEntry>,
    var_stack: Vec<VarEntry>,
    scope_level: u32,
    current_function: Option<String>,
    current_return_type: NwType,
    // Struct type-name of the current function's return type (for -620 name match).
    current_return_struct_name: Option<String>,
    collect_all_errors: bool,
    require_entry_point: bool,
    loop_depth: u32,
    switch_depth: u32,
    // Recursion-depth guard for expression checking — a pathologically long operator
    // chain (`a+a+a+…` thousands deep) would otherwise overflow the stack.
    expr_depth: u32,
}

impl<'a> SemanticChecker<'a> {
    pub fn new(arena: &'a AstArena, file_names: &'a [String]) -> Self {
        // C++ scriptcompcore.cpp:925-950 (InitializePreDefinedStructures) seeds the
        // struct table with the "vector" pseudo-struct so `struct vector v;` resolves
        // identically to the `vector` keyword. Match.
        let vector_struct = StructDef {
            name: "vector".to_string(),
            fields: vec![
                FieldInfo { name: "x".to_string(), nw_type: NwType::Float, type_name: None, offset: 0 },
                FieldInfo { name: "y".to_string(), nw_type: NwType::Float, type_name: None, offset: 4 },
                FieldInfo { name: "z".to_string(), nw_type: NwType::Float, type_name: None, offset: 8 },
            ],
            byte_size: 12,
        };
        Self {
            arena,
            file_names,
            diagnostics: Vec::new(),
            functions: Vec::new(),
            structs: vec![vector_struct],
            globals: Vec::new(),
            var_stack: Vec::new(),
            scope_level: 0,
            current_function: None,
            current_return_type: NwType::Void,
            current_return_struct_name: None,
            collect_all_errors: false,
            require_entry_point: true,
            loop_depth: 0,
            switch_depth: 0,
            expr_depth: 0,
        }
    }

    pub fn set_collect_all_errors(&mut self, v: bool) {
        self.collect_all_errors = v;
    }

    pub fn set_require_entry_point(&mut self, v: bool) {
        self.require_entry_point = v;
    }

    pub fn load_lang_spec(&mut self, spec: &str) {
        let cleaned = preprocess_lang_spec(spec);

        let mut lexer = crate::lexer::Lexer::new(&cleaned, "nwscript.nss", 0);
        let tokens = lexer.tokenize();
        let mut parser = crate::parser::Parser::new(tokens);
        parser.file_names.push("nwscript.nss".to_string());
        parser.set_collect_all_errors(true);
        parser.set_require_entry_point(false);
        if let Ok(root) = parser.parse_program() {
            self.collect_from_arena(root, &parser.arena, true);
        }
    }

    pub fn load_included_file(&mut self, root: NodeId, arena: &AstArena, file_names: &[String]) {
        self.collect_from_arena(root, arena, false);

        // Type-check function bodies in the included file using a temporary
        // checker that shares our symbol tables (functions, structs, globals).
        let mut sub = SemanticChecker::new(arena, file_names);
        sub.collect_all_errors = self.collect_all_errors;
        sub.require_entry_point = false;
        sub.functions = self.functions.clone();
        sub.structs = self.structs.clone();
        sub.globals = self.globals.clone();

        let _ = sub.check_pass(root);

        self.diagnostics.extend(sub.diagnostics);
    }

    fn collect_from_arena(&mut self, root: NodeId, arena: &AstArena, is_engine: bool) {
        // Iterative walk to avoid stack overflow on large files (nwscript.nss has 6000+ declarations)
        let mut stack = vec![root];
        while let Some(node_id) = stack.pop() {
            if node_id == NULL_NODE { continue; }
            let node = arena.get(node_id).clone();

            match node.op {
                Operation::FunctionalUnit => {
                    // Push right first so left is processed first
                    if node.right != NULL_NODE { stack.push(node.right); }
                    if node.left != NULL_NODE { stack.push(node.left); }
                }
                Operation::FunctionDeclaration => {
                    self.register_func_from_arena(node.left, false, arena, is_engine);
                }
                Operation::Function => {
                    self.register_func_from_arena(node.left, true, arena, is_engine);
                }
                Operation::KeywordStruct => {
                    if node.left != NULL_NODE {
                        let def = arena.get(node.left).clone();
                        if def.op == Operation::StructureDefinition {
                            self.register_struct_from_arena(&def, arena);
                        }
                    }
                }
                Operation::GlobalVariables => {
                    self.register_global_from_arena(&node, arena);
                }
                Operation::ConstDeclaration => {
                    let name = node.string_data.as_deref().unwrap_or("").to_string();
                    let const_value = if node.left != NULL_NODE {
                        Self::extract_default_value_with_globals(arena, node.left, Some(&self.globals))
                    } else { None };
                    self.globals.push(VarEntry {
                        name,
                        nw_type: node.nw_type,
                        type_name: node.type_name.clone(),
                        scope_level: 0,
                        is_constant: true,
                        const_value,
                    });
                }
                _ => {}
            }
        }
    }

    fn register_struct_from_arena(&mut self, def_node: &AstNode, arena: &AstArena) {
        let name = def_node.string_data.as_deref().unwrap_or("").to_string();
        let mut fields = Vec::new();
        let mut offset = 0;
        let mut field_chain = def_node.left;

        while field_chain != NULL_NODE {
            let vl = arena.get(field_chain).clone();
            if vl.left != NULL_NODE {
                let var = arena.get(vl.left).clone();
                let field_name = var.string_data.as_deref().unwrap_or("").to_string();

                // C++ scriptcompfinalcode.cpp:4730-4768: a self-referential struct field
                // contributes 0 bytes (the type isn't in the struct list yet). The struct
                // is only rejected if its TOTAL size ends up 0 (handled below).
                let is_self_ref = var.nw_type == NwType::Struct
                    && var.type_name.as_deref() == Some(name.as_str());
                let size = if is_self_ref { 0 } else { self.field_size(var.nw_type, &var.type_name) };
                fields.push(FieldInfo {
                    name: field_name,
                    nw_type: var.nw_type,
                    type_name: var.type_name.clone(),
                    offset,
                });
                offset += size;
            }
            field_chain = vl.right;
        }

        // C++ rejects a struct with total byte size 0 (UNKNOWN_STATE_IN_COMPILER).
        if offset == 0 {
            let _ = self.error_at(CompileError::UnknownStateInCompiler, def_node);
        }
        self.structs.push(StructDef { name, fields, byte_size: offset });
    }

    fn field_size(&self, nw_type: NwType, type_name: &Option<String>) -> i32 {
        match nw_type {
            NwType::Struct => {
                if let Some(tn) = type_name {
                    if let Some(sd) = self.structs.iter().find(|s| s.name == *tn) {
                        return sd.byte_size;
                    }
                }
                0
            }
            NwType::Vector => 12,
            _ => nw_type.size_bytes(),
        }
    }

    fn register_global_from_arena(&mut self, node: &AstNode, arena: &AstArena) {
        if node.left == NULL_NODE {
            return;
        }
        let decl = arena.get(node.left).clone();
        if decl.left == NULL_NODE {
            return;
        }
        let type_node = arena.get(decl.left).clone();
        let nw_type = op_to_nw_type(type_node.op);
        let type_name = type_node.type_name.clone();

        let mut vl_id = type_node.left;
        while vl_id != NULL_NODE {
            let vl = arena.get(vl_id).clone();
            if vl.left != NULL_NODE {
                let var = arena.get(vl.left).clone();
                let var_name = var.string_data.as_deref().unwrap_or("").to_string();
                // Even non-const globals (like nwscript.nss's `int TRUE = 1;`)
                // can be used as parameter defaults — capture their initializer.
                let const_value = if var.left != NULL_NODE {
                    Self::extract_default_value_with_globals(arena, var.left, Some(&self.globals))
                } else { None };
                self.globals.push(VarEntry {
                    name: var_name,
                    nw_type,
                    type_name: type_name.clone(),
                    scope_level: 0,
                    is_constant: false,
                    const_value,
                });
            }
            vl_id = vl.right;
        }
    }

    fn register_func_from_arena(&mut self, func_id_node: NodeId, has_impl: bool, arena: &AstArena, is_engine: bool) {
        if func_id_node == NULL_NODE {
            return;
        }
        let fid = arena.get(func_id_node).clone();
        let name = fid.string_data.as_deref().unwrap_or("").to_string();

        let mut params = Vec::new();
        let mut param_node = fid.left;
        while param_node != NULL_NODE {
            let p = arena.get(param_node).clone();
            if p.op == Operation::FunctionParamName {
                let default_value = if p.left != NULL_NODE {
                    Self::extract_default_value_with_globals(arena, p.left, Some(&self.globals))
                } else { None };
                params.push(ParamInfo {
                    name: p.string_data.as_deref().unwrap_or("").to_string(),
                    nw_type: p.nw_type,
                    type_name: p.type_name.clone(),
                    has_default: p.left != NULL_NODE,
                    default_value,
                });
            }
            param_node = p.right;
        }

        if let Some(existing) = self.functions.iter_mut().find(|f| f.name == name) {
            if has_impl {
                existing.has_implementation = true;
            }
        } else {
            // C++ assigns each engine action a sequential ID matching its declaration order
            // in nwscript.nss (m_nPredefinedIdentifierOrder).
            let action_id = if is_engine {
                self.functions.iter().filter(|f| f.is_engine_action).count() as u32
            } else { 0 };
            self.functions.push(FunctionSig {
                name,
                return_type: fid.nw_type,
                return_type_name: fid.type_name.clone(),
                params,
                has_implementation: has_impl,
                is_engine_action: is_engine,
                action_id,
            });
        }
    }

    fn error_at(&mut self, err: CompileError, node: &AstNode) -> Result<(), CompileError> {
        let file = if (node.file_id as usize) < self.file_names.len() {
            self.file_names[node.file_id as usize].clone()
        } else {
            "<unknown>".to_string()
        };
        self.diagnostics.push(Diagnostic {
            error: err,
            severity: err.default_severity(),
            file,
            line: node.line,
            message: err.message().to_string(),
        });
        if self.collect_all_errors {
            Ok(())
        } else {
            Err(err)
        }
    }

    fn error_with_msg(
        &mut self,
        err: CompileError,
        node: &AstNode,
        extra: &str,
    ) -> Result<(), CompileError> {
        let file = if (node.file_id as usize) < self.file_names.len() {
            self.file_names[node.file_id as usize].clone()
        } else {
            "<unknown>".to_string()
        };
        let msg = if extra.is_empty() {
            err.message().to_string()
        } else {
            format!("{}: {}", err.message(), extra)
        };
        self.diagnostics.push(Diagnostic {
            error: err,
            severity: err.default_severity(),
            file,
            line: node.line,
            message: msg,
        });
        if self.collect_all_errors {
            Ok(())
        } else {
            Err(err)
        }
    }

    pub fn check(&mut self, root: NodeId) -> Result<(), CompileError> {
        self.collect_pass(root);
        self.check_pass(root)?;

        if self.require_entry_point {
            let main_fn = self.functions.iter().find(|f| f.name == "main" && f.has_implementation).cloned();
            let intsc_fn = self.functions.iter().find(|f| f.name == "StartingConditional" && f.has_implementation).cloned();

            match (&main_fn, &intsc_fn) {
                (Some(m), _) => {
                    // C++: main must be void and take no parameters
                    let root_node = self.arena.get(root).clone();
                    if m.return_type != NwType::Void {
                        let _ = self.error_at(CompileError::FunctionMainMustHaveVoidReturnValue, &root_node);
                    }
                    if !m.params.is_empty() {
                        let _ = self.error_at(CompileError::FunctionMainMustHaveNoParameters, &root_node);
                    }
                }
                (None, Some(s)) => {
                    // C++: StartingConditional must return int and take no parameters
                    let root_node = self.arena.get(root).clone();
                    if s.return_type != NwType::Integer {
                        let _ = self.error_at(CompileError::FunctionIntscMustHaveVoidReturnValue, &root_node);
                    }
                    if !s.params.is_empty() {
                        let _ = self.error_at(CompileError::FunctionIntscMustHaveNoParameters, &root_node);
                    }
                }
                (None, None) => {
                    let node = self.arena.get(root);
                    self.error_at(CompileError::NoFunctionMainInScript, node)?;
                }
            }
        }

        for func in &self.functions {
            if !func.has_implementation && !func.is_engine_action {
                // Forward declaration without implementation — not an error by itself.
                // Only matters if it's actually called.
            }
        }

        Ok(())
    }

    fn collect_pass(&mut self, root: NodeId) {
        let mut stack = vec![root];
        while let Some(node_id) = stack.pop() {
            if node_id == NULL_NODE { continue; }
            let node = self.arena.get(node_id).clone();

            match node.op {
                Operation::FunctionalUnit => {
                    if node.right != NULL_NODE { stack.push(node.right); }
                    if node.left != NULL_NODE { stack.push(node.left); }
                }
                Operation::KeywordStruct => {
                    if node.left != NULL_NODE {
                        let def = self.arena.get(node.left).clone();
                        if def.op == Operation::StructureDefinition {
                            self.register_struct(&def);
                        }
                    }
                }
                Operation::FunctionDeclaration => {
                    self.register_func_decl(node.left, false);
                }
                Operation::Function => {
                    self.register_func_decl(node.left, true);
                }
                Operation::GlobalVariables => {
                    self.register_global(&node);
                }
                Operation::ConstDeclaration => {
                    self.register_const_global(&node);
                }
                _ => {}
            }
        }
    }

    fn register_struct(&mut self, def_node: &AstNode) {
        let name = def_node
            .string_data
            .as_deref()
            .unwrap_or("")
            .to_string();

        // Detect struct redefinition (C++ matches by name)
        if self.structs.iter().any(|s| s.name == name) {
            let _ = self.error_at(CompileError::StructureRedefined, def_node);
        }

        let mut fields: Vec<FieldInfo> = Vec::new();
        let mut offset = 0;
        let mut field_chain = def_node.left;

        while field_chain != NULL_NODE {
            let vl = self.arena.get(field_chain).clone();
            if vl.left != NULL_NODE {
                let var = self.arena.get(vl.left).clone();
                let field_name = var.string_data.as_deref().unwrap_or("").to_string();

                // C++: self-referential field contributes 0 bytes; struct is rejected
                // only if total size ends up 0 (checked at end of register_struct).
                let is_self_ref = var.nw_type == NwType::Struct
                    && var.type_name.as_deref() == Some(name.as_str());

                // Detect duplicate field names
                if fields.iter().any(|f| f.name == field_name) {
                    let _ = self.error_at(CompileError::VariableUsedTwiceInSameStructure, &var);
                }

                let size = if is_self_ref { 0 } else { self.field_size(var.nw_type, &var.type_name) };
                fields.push(FieldInfo {
                    name: field_name,
                    nw_type: var.nw_type,
                    type_name: var.type_name.clone(),
                    offset,
                });
                offset += size;
            }
            field_chain = vl.right;
        }

        if offset == 0 {
            let _ = self.error_at(CompileError::UnknownStateInCompiler, def_node);
        }
        self.structs.push(StructDef {
            name,
            fields,
            byte_size: offset,
        });
    }

    fn register_func_decl(&mut self, func_id_node: NodeId, has_impl: bool) {
        if func_id_node == NULL_NODE {
            return;
        }
        let fid = self.arena.get(func_id_node).clone();
        let name = fid.string_data.as_deref().unwrap_or("").to_string();

        let mut params = Vec::new();
        let mut param_node = fid.left;
        while param_node != NULL_NODE {
            let p = self.arena.get(param_node).clone();
            if p.op == Operation::FunctionParamName {
                let pname = p.string_data.as_deref().unwrap_or("").to_string();
                // Detect duplicate parameter names
                if params.iter().any(|existing: &ParamInfo| existing.name == pname) {
                    let _ = self.error_at(CompileError::VariableAlreadyUsedWithinScope, &p);
                }
                // C++: per scriptcompparsetree.cpp:4053-4302, optional parameters are
                // restricted by parameter type:
                //   - Struct types other than `vector` reject any default
                //   - EngineStructure: only location (id 2) and json (id 7) can take defaults
                //   - vector defaults must be vector literals; location must be ConstantLocation,
                //     json must be ConstantJson
                if p.left != NULL_NODE {
                    // Types that cannot have ANY default value. C++ hard-returns after
                    // this error (scriptcompparsetree.cpp:4053-4060), so do NOT also run
                    // the literal-shape check below — that would emit a spurious -630.
                    let type_forbids_default = match p.nw_type {
                        NwType::Struct => p.type_name.as_deref() != Some("vector"),
                        NwType::EngineStructure(n) => !matches!(n, 2 | 7),
                        _ => false,
                    };
                    if type_forbids_default {
                        let _ = self.error_at(CompileError::TypeDoesNotHaveAnOptionalParameter, &p);
                    } else {
                    // Validate the default value's AST shape against the parameter type
                    // (C++ scriptcompparsetree.cpp:4082-4242 checks the literal kind
                    // BEFORE identifier folding).
                    let default_node = self.arena.get(p.left).clone();
                    // C++ validates the default value's node shape BEFORE folding, so an
                    // operator expression like `2 * 3` is rejected even though it folds to
                    // a constant (unary `-literal` stays valid). fold_constants marks such
                    // folded operator results; reject them with -630.
                    if !default_node.allow_as_default_value {
                        let _ = self.error_at(CompileError::NonConstantInFunctionDeclaration, &p);
                    }
                    let shape_ok = match p.nw_type {
                        NwType::Integer => matches!(default_node.op, Operation::ConstantInteger),
                        NwType::Float => matches!(default_node.op, Operation::ConstantFloat),
                        NwType::String => matches!(default_node.op, Operation::ConstantString),
                        NwType::Object => matches!(default_node.op, Operation::ConstantObject),
                        NwType::Vector | NwType::Struct => matches!(default_node.op, Operation::ConstantVector),
                        NwType::EngineStructure(2) => matches!(default_node.op, Operation::ConstantLocation),
                        NwType::EngineStructure(7) => matches!(default_node.op, Operation::ConstantJson),
                        _ => true,
                    };
                    // A bare identifier (engine constant like OBJECT_INVALID / TRUE) is
                    // accepted — its value is folded at codegen time.
                    let is_literal_const = matches!(
                        default_node.op,
                        Operation::ConstantInteger
                            | Operation::ConstantFloat
                            | Operation::ConstantString
                            | Operation::ConstantObject
                            | Operation::ConstantVector
                            | Operation::ConstantLocation
                            | Operation::ConstantJson
                    );
                    if !shape_ok {
                        if is_literal_const {
                            // Wrong-typed literal default (e.g. `int a = 1.0`).
                            let _ = self.error_at(CompileError::NonConstantInFunctionDeclaration, &p);
                        } else if default_node.op != Operation::Variable
                            && !self.is_constant_expression(p.left)
                        {
                            // Non-constant expression default (e.g. a function call).
                            let _ = self.error_at(CompileError::NonConstantInFunctionDeclaration, &p);
                        }
                    }
                    }
                }
                let default_value = if p.left != NULL_NODE {
                    Self::extract_default_value_with_globals(self.arena, p.left, Some(&self.globals))
                } else { None };
                params.push(ParamInfo {
                    name: pname,
                    nw_type: p.nw_type,
                    type_name: p.type_name.clone(),
                    has_default: p.left != NULL_NODE,
                    default_value,
                });
            }
            param_node = p.right;
        }

        let new_return = fid.nw_type;
        let new_return_name = fid.type_name.clone();

        let existing_idx = self.functions.iter().position(|f| f.name == name);
        if let Some(idx) = existing_idx {
            // C++ only compares parameter list (not return type) for decl-vs-impl match
            let return_matches = true;
            let _ = new_return; let _ = new_return_name;
            let params_match = self.functions[idx].params.len() == params.len()
                && self.functions[idx].params.iter().zip(params.iter())
                    .all(|(a, b)| a.nw_type == b.nw_type && a.type_name == b.type_name);

            if !return_matches || !params_match {
                let _ = self.error_at(
                    CompileError::FunctionImplementationAndDefinitionDiffer,
                    &fid,
                );
                return;
            }

            let already_implemented = self.functions[idx].has_implementation;
            let is_engine_action = self.functions[idx].is_engine_action;
            if has_impl {
                // C++ scriptcompparsetree.cpp:4350-4358: engine actions are pre-flagged
                // as "implementation in place" — a user redefining one is a duplicate.
                if already_implemented || is_engine_action {
                    let _ = self.error_at(
                        CompileError::DuplicateFunctionImplementation,
                        &fid,
                    );
                }
                self.functions[idx].has_implementation = true;
            }
        } else {
            self.functions.push(FunctionSig {
                name,
                return_type: new_return,
                return_type_name: new_return_name,
                params,
                has_implementation: has_impl,
                is_engine_action: false,
                action_id: 0,
            });
        }
    }

    fn register_global(&mut self, node: &AstNode) {
        // GlobalVariables -> KeywordDeclaration -> type_node -> VariableList -> Variable
        if node.left == NULL_NODE {
            return;
        }
        let decl = self.arena.get(node.left).clone();
        if decl.left == NULL_NODE {
            return;
        }
        let type_node = self.arena.get(decl.left).clone();
        let nw_type = op_to_nw_type(type_node.op);
        let type_name = type_node.type_name.clone();

        let mut vl_id = type_node.left;
        while vl_id != NULL_NODE {
            let vl = self.arena.get(vl_id).clone();
            if vl.left != NULL_NODE {
                let var = self.arena.get(vl.left).clone();
                let var_name = var.string_data.as_deref().unwrap_or("").to_string();

                // C++ scriptcompfinalcode.cpp:2767-2773 emits
                // VARIABLE_ALREADY_USED_WITHIN_SCOPE for any duplicate global / const name.
                if self.globals.iter().any(|g| g.name == var_name) {
                    let _ = self.error_at(CompileError::VariableAlreadyUsedWithinScope, &var);
                }

                // Check initializer type matches declared type
                if var.left != NULL_NODE {
                    if let Some(init_type) = self.infer_const_type(var.left) {
                        if init_type != nw_type
                            && init_type != NwType::Void
                            && nw_type != NwType::Void
                        {
                            let _ = self.error_at(CompileError::MismatchedTypes, &var);
                        }
                    }
                    // C++ scriptcompfinalcode.cpp:5940-5965: a global initializer can
                    // only reference identifiers added earlier — forward references like
                    // `int b = a; int a = 5;` raise UndefinedIdentifier.
                    self.check_initializer_refs(var.left);
                }

                let const_value = if var.left != NULL_NODE {
                    Self::extract_default_value_with_globals(self.arena, var.left, Some(&self.globals))
                } else { None };
                self.globals.push(VarEntry {
                    name: var_name,
                    nw_type,
                    type_name: type_name.clone(),
                    scope_level: 0,
                    is_constant: false,
                    const_value,
                });
            }
            vl_id = vl.right;
        }
    }

    fn check_initializer_refs(&mut self, node_id: NodeId) {
        // Explicit-stack walk: a global initializer is NOT routed through the
        // depth-guarded check_expression, so a deep operator chain (`1+1+...`) here
        // would otherwise overflow the stack (hard crash, bricks the WASM instance).
        let mut stack = vec![node_id];
        while let Some(id) = stack.pop() {
            if id == NULL_NODE { continue; }
            let n = self.arena.get(id).clone();
            if n.op == Operation::Variable {
                let name = n.string_data.as_deref().unwrap_or("");
                // Resolve against globals registered so far (earlier in source order).
                // Functions appear as Action nodes, not bare Variable references.
                if !self.globals.iter().any(|v| v.name == name) {
                    let _ = self.error_with_msg(CompileError::UndefinedIdentifier, &n, name);
                }
                continue;
            }
            if n.left != NULL_NODE { stack.push(n.left); }
            if n.right != NULL_NODE { stack.push(n.right); }
        }
    }

    fn register_const_global(&mut self, node: &AstNode) {
        let name = node.string_data.as_deref().unwrap_or("").to_string();
        let declared_type = node.nw_type;

        if self.globals.iter().any(|g| g.name == name) {
            let _ = self.error_at(CompileError::VariableAlreadyUsedWithinScope, node);
        }

        // Check initializer type matches declared type
        if node.left != NULL_NODE {
            let init_type = match self.infer_const_type(node.left) {
                Some(t) => t,
                None => declared_type,
            };
            if init_type != declared_type
                && init_type != NwType::Void
                && declared_type != NwType::Void
            {
                let _ = self.error_at(CompileError::MismatchedTypes, node);
            }
            // C++ scriptcompfinalcode.cpp:1252-1276 requires the const-global
            // initializer to be a literal (after folding), not a function call or
            // other non-constant expression.
            if !self.is_constant_expression(node.left) {
                let _ = self.error_at(CompileError::InvalidValueAssignedToConstant, node);
            }
        }

        let const_value = if node.left != NULL_NODE {
            Self::extract_default_value_with_globals(self.arena, node.left, Some(&self.globals))
        } else { None };
        self.globals.push(VarEntry {
            name,
            nw_type: declared_type,
            type_name: node.type_name.clone(),
            scope_level: 0,
            is_constant: true,
            const_value,
        });
    }

    /// Infer the type of a constant initializer expression without needing
    /// the full check_expression machinery (which mutates state).
    fn infer_const_type(&self, node_id: NodeId) -> Option<NwType> {
        if node_id == NULL_NODE { return None; }
        let node = self.arena.get(node_id);
        match node.op {
            Operation::ConstantInteger => Some(NwType::Integer),
            Operation::ConstantFloat => Some(NwType::Float),
            Operation::ConstantString => Some(NwType::String),
            Operation::ConstantObject => Some(NwType::Object),
            Operation::ConstantVector => Some(NwType::Vector),
            Operation::ConstantJson => Some(NwType::EngineStructure(7)),
            Operation::ConstantLocation => Some(NwType::EngineStructure(2)),
            Operation::Negation => self.infer_const_type(node.left),
            _ => None,
        }
    }

    fn check_pass(&mut self, root: NodeId) -> Result<(), CompileError> {
        let mut stack = vec![root];
        while let Some(node_id) = stack.pop() {
            if node_id == NULL_NODE { continue; }
            let node = self.arena.get(node_id).clone();

            match node.op {
                Operation::FunctionalUnit => {
                    if node.right != NULL_NODE { stack.push(node.right); }
                    if node.left != NULL_NODE { stack.push(node.left); }
                }
                Operation::Function => {
                    self.check_function(node_id)?;
                }
                Operation::FunctionDeclaration
                | Operation::KeywordStruct
                | Operation::GlobalVariables
                | Operation::ConstDeclaration => {}
                _ => {}
            }
        }
        Ok(())
    }

    fn check_function(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        let func_id_node = node.left;

        if func_id_node == NULL_NODE {
            return Ok(());
        }

        let fid = self.arena.get(func_id_node).clone();
        let func_name = fid.string_data.as_deref().unwrap_or("").to_string();

        self.current_function = Some(func_name);
        self.current_return_type = fid.nw_type;
        self.current_return_struct_name = fid.type_name.clone();

        // Push parameters as local variables
        let saved_var_count = self.var_stack.len();
        self.scope_level += 1;

        let mut param_node = fid.left;
        while param_node != NULL_NODE {
            let p = self.arena.get(param_node).clone();
            if p.op == Operation::FunctionParamName {
                self.var_stack.push(VarEntry {
                    name: p.string_data.as_deref().unwrap_or("").to_string(),
                    nw_type: p.nw_type,
                    type_name: p.type_name.clone(),
                    scope_level: self.scope_level,
                    is_constant: false,
                    const_value: None,
                });
            }
            param_node = p.right;
        }

        // Check function body
        let body = node.right;
        if body != NULL_NODE {
            match self.check_statement(body) {
                Ok(()) => {}
                Err(e) => {
                    if !self.collect_all_errors {
                        self.var_stack.truncate(saved_var_count);
                        self.scope_level -= 1;
                        self.current_function = None;
                        return Err(e);
                    }
                }
            }
        }

        // Check all paths return for non-void functions
        if fid.nw_type != NwType::Void && body != NULL_NODE {
            if !self.all_paths_return(body) {
                let _ = self.error_at(
                    CompileError::NotAllControlPathsReturnAValue,
                    &fid,
                );
            }
        }

        self.var_stack.truncate(saved_var_count);
        self.scope_level -= 1;
        self.current_function = None;

        Ok(())
    }

    fn all_paths_return(&self, node_id: NodeId) -> bool {
        if node_id == NULL_NODE { return false; }
        let node = self.arena.get(node_id);

        match node.op {
            Operation::Return => true,
            Operation::CompoundStatement => self.all_paths_return(node.left),
            Operation::StatementList => {
                // If any statement in the chain returns, the chain returns. Iterate the
                // right-spine (recursing it overflows on huge bodies); recurse per stmt.
                let mut cur = node_id;
                while cur != NULL_NODE {
                    let n = self.arena.get(cur);
                    if n.op != Operation::StatementList {
                        return self.all_paths_return(cur);
                    }
                    if self.all_paths_return(n.left) {
                        return true;
                    }
                    cur = n.right;
                }
                false
            }
            Operation::Statement | Operation::StatementNoDebug => {
                self.all_paths_return(node.left)
            }
            Operation::IfBlock => {
                // Both then and else branches must return
                if node.right == NULL_NODE { return false; }
                let choice = self.arena.get(node.right);
                if choice.right == NULL_NODE {
                    // No else branch — could fall through
                    return false;
                }
                self.all_paths_return(choice.left) && self.all_paths_return(choice.right)
            }
            // C++ `FoundReturnStatementOnAllBranches` (scriptcompfinalcode.cpp:5829) does NOT
            // count a switch as a returning branch — even with a default and every case returning.
            Operation::SwitchBlock => false,
            _ => false,
        }
    }

    fn switch_all_paths_return(&self, node_id: NodeId) -> bool {
        // Flatten the switch body and check for default + all-return
        let mut has_default = false;
        let mut all_return = true;
        let mut in_case = false;
        let mut case_has_return = false;
        let mut items = Vec::new();
        self.flatten_stmt_list(node_id, &mut items);

        for &item in &items {
            let n = self.arena.get(item);
            match n.op {
                Operation::Case => {
                    if in_case && !case_has_return {
                        all_return = false;
                    }
                    in_case = true;
                    case_has_return = false;
                }
                Operation::Default => {
                    if in_case && !case_has_return {
                        all_return = false;
                    }
                    has_default = true;
                    in_case = true;
                    case_has_return = false;
                }
                _ => {
                    if in_case && self.all_paths_return(item) {
                        case_has_return = true;
                    }
                }
            }
        }
        if in_case && !case_has_return { all_return = false; }
        has_default && all_return
    }

    fn flatten_stmt_list(&self, node_id: NodeId, out: &mut Vec<NodeId>) {
        // Explicit-stack in-order flatten (recursion overflows on long switch bodies).
        // Push right then left so left is processed first, preserving left→right order.
        let mut stack = vec![node_id];
        while let Some(id) = stack.pop() {
            if id == NULL_NODE { continue; }
            let node = self.arena.get(id);
            if node.op == Operation::StatementList {
                stack.push(node.right);
                stack.push(node.left);
            } else {
                out.push(id);
            }
        }
    }

    fn check_statement(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        if node_id == NULL_NODE {
            return Ok(());
        }
        let node = self.arena.get(node_id).clone();

        match node.op {
            Operation::CompoundStatement => {
                self.scope_level += 1;
                let saved = self.var_stack.len();
                self.check_statement(node.left)?;
                self.var_stack.truncate(saved);
                self.scope_level -= 1;
            }
            Operation::StatementList => {
                // Iterate the right-linked chain; recurse only into each statement.
                // Recursing the chain overflows the stack on huge function bodies.
                let mut cur = node_id;
                while cur != NULL_NODE {
                    let n = self.arena.get(cur).clone();
                    if n.op != Operation::StatementList {
                        self.check_statement(cur)?;
                        break;
                    }
                    self.check_statement(n.left)?;
                    cur = n.right;
                }
            }
            Operation::Statement | Operation::StatementNoDebug => {
                self.check_statement(node.left)?;
            }
            Operation::KeywordDeclaration | Operation::ConstDeclaration => {
                self.check_local_declaration(node_id)?;
            }
            Operation::IfBlock => {
                self.check_if(node_id)?;
            }
            Operation::WhileBlock => {
                self.check_while(node_id)?;
            }
            Operation::DoWhileBlock => {
                self.check_do_while(node_id)?;
            }
            Operation::ForBlock => {
                self.check_statement(node.left)?;
            }
            Operation::SwitchBlock => {
                self.check_switch(node_id)?;
            }
            Operation::Return => {
                if node.left != NULL_NODE {
                    let ret_type = self.check_expression(node.left)?;
                    if self.current_return_type == NwType::Void {
                        self.error_at(
                            CompileError::ReturnTypeAndFunctionTypeMismatched,
                            &node,
                        )?;
                    } else if ret_type != self.current_return_type {
                        // Match C++: no implicit conversion, void RHS is also a mismatch
                        self.error_at(
                            CompileError::ReturnTypeAndFunctionTypeMismatched,
                            &node,
                        )?;
                    } else if ret_type == NwType::Struct {
                        // Both sides are structs: C++ (scriptcompfinalcode.cpp:5461) also
                        // requires the struct *type names* to match, else -620.
                        let ret_struct = self.resolve_struct_name(node.left);
                        if ret_struct.is_some()
                            && ret_struct != self.current_return_struct_name
                        {
                            self.error_at(
                                CompileError::ReturnTypeAndFunctionTypeMismatched,
                                &node,
                            )?;
                        }
                    }
                } else {
                    // bare `return;` in a non-void function — C++ reuses
                    // ReturnTypeAndFunctionTypeMismatched (-620) for both directions.
                    if self.current_return_type != NwType::Void {
                        self.error_at(
                            CompileError::ReturnTypeAndFunctionTypeMismatched,
                            &node,
                        )?;
                    }
                }
            }
            Operation::Break => {
                if self.loop_depth == 0 && self.switch_depth == 0 {
                    self.error_at(
                        CompileError::BreakOutsideOfLoopOrCaseStatement,
                        &node,
                    )?;
                }
            }
            Operation::Continue => {
                if self.loop_depth == 0 {
                    self.error_at(
                        CompileError::BreakOutsideOfLoopOrCaseStatement,
                        &node,
                    )?;
                }
            }
            _ => {
                // Expression statement
                self.check_expression(node_id)?;
            }
        }

        Ok(())
    }

    fn check_local_declaration(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        // Walk through type node -> variable list
        let type_node_id = node.left;
        if type_node_id == NULL_NODE {
            return Ok(());
        }
        let type_node = self.arena.get(type_node_id).clone();
        let decl_type = op_to_nw_type(type_node.op);
        let type_name = type_node.type_name.clone();

        let mut vl_id = type_node.left;
        while vl_id != NULL_NODE {
            let vl = self.arena.get(vl_id).clone();
            if vl.left != NULL_NODE {
                let var = self.arena.get(vl.left).clone();
                let var_name = var.string_data.as_deref().unwrap_or("").to_string();

                // Check for duplicate in same scope
                let dup = self.var_stack.iter().any(|v| {
                    v.name == var_name && v.scope_level == self.scope_level
                });
                if dup {
                    self.error_with_msg(
                        CompileError::VariableAlreadyUsedWithinScope,
                        &var,
                        &var_name,
                    )?;
                }

                // Check initializer type
                if var.left != NULL_NODE {
                    let init_type = self.check_expression(var.left)?;
                    if init_type == NwType::Void && decl_type != NwType::Void {
                        // C++ scriptcompfinalcode.cpp:3897-3900 — void result in a
                        // non-void context raises VoidExpressionWhereNonVoidRequired.
                        self.error_at(CompileError::VoidExpressionWhereNonVoidRequired, &var)?;
                    } else if init_type != decl_type
                        && init_type != NwType::Void
                        && decl_type != NwType::Void
                    {
                        self.error_at(CompileError::MismatchedTypes, &var)?;
                    }
                }

                self.var_stack.push(VarEntry {
                    name: var_name,
                    nw_type: decl_type,
                    type_name: type_name.clone(),
                    scope_level: self.scope_level,
                    is_constant: node.op == Operation::ConstDeclaration,
                    const_value: None,
                });
            }
            vl_id = vl.right;
        }

        Ok(())
    }

    fn check_if(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        if node.left != NULL_NODE {
            let cond = self.arena.get(node.left).clone();
            if cond.left != NULL_NODE {
                let t = self.check_expression(cond.left)?;
                self.require_integer_cond(t, &cond)?;
            }
        }
        if node.right != NULL_NODE {
            let choice = self.arena.get(node.right).clone();
            self.check_statement(choice.left)?;
            if choice.right != NULL_NODE {
                self.check_statement(choice.right)?;
            }
        }
        Ok(())
    }

    fn check_while(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        if node.left != NULL_NODE {
            let cond = self.arena.get(node.left).clone();
            if cond.left != NULL_NODE {
                let t = self.check_expression(cond.left)?;
                self.require_integer_cond(t, &cond)?;
            }
        }
        self.loop_depth += 1;
        if node.right != NULL_NODE {
            let choice = self.arena.get(node.right).clone();
            self.check_statement(choice.left)?;
        }
        self.loop_depth -= 1;
        Ok(())
    }

    fn require_integer_cond(&mut self, t: NwType, node: &AstNode) -> Result<(), CompileError> {
        // C++: conditions must evaluate to integer (not void, not float, not anything else)
        if t == NwType::Void {
            let already = self.diagnostics.iter().any(|d| d.line == node.line);
            if !already {
                self.error_at(CompileError::VoidExpressionWhereNonVoidRequired, node)?;
            }
        } else if t != NwType::Integer {
            self.error_at(CompileError::NonIntegerExpressionWhereIntegerRequired, node)?;
        }
        Ok(())
    }

    /// Both operands of a logical/bitwise/shift operator must be integer. C++ rejects
    /// a void operand (e.g. a void function-call result) here too — a void operand is
    /// not wrapped in NON_VOID_EXPRESSION so it reaches the operator's INT check.
    /// Cascade noise (an operand that already errored, e.g. an undefined identifier
    /// resolving to Void) is suppressed via the per-line dedup the codebase uses.
    fn require_integer_operands(
        &mut self,
        lt: NwType,
        rt: NwType,
        err: CompileError,
        node: &AstNode,
    ) -> Result<(), CompileError> {
        if lt != NwType::Integer || rt != NwType::Integer {
            let cascade = (lt == NwType::Void || rt == NwType::Void)
                && self.diagnostics.iter().any(|d| d.line == node.line);
            if !cascade {
                self.error_at(err, node)?;
            }
        }
        Ok(())
    }

    fn check_do_while(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        self.loop_depth += 1;
        self.check_statement(node.left)?;
        self.loop_depth -= 1;
        if node.right != NULL_NODE {
            let cond = self.arena.get(node.right).clone();
            if cond.left != NULL_NODE {
                let t = self.check_expression(cond.left)?;
                self.require_integer_cond(t, &cond)?;
            }
        }
        Ok(())
    }

    fn check_switch(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        if node.left != NULL_NODE {
            let cond = self.arena.get(node.left).clone();
            if cond.left != NULL_NODE {
                let switch_type = self.check_expression(cond.left)?;
                if switch_type != NwType::Integer {
                    self.error_at(CompileError::SwitchMustEvaluateToAnInteger, &cond)?;
                }
            }
        }
        self.switch_depth += 1;

        // Collect case values and detect duplicates + multiple defaults
        let mut seen_cases: Vec<i32> = Vec::new();
        let mut seen_default = false;
        self.check_switch_cases(node.right, &mut seen_cases, &mut seen_default)?;

        // C++: case label cannot jump over a local declaration inside the switch body
        let mut items = Vec::new();
        self.flatten_stmt_list(node.right, &mut items);
        let mut decl_seen_after_case = false;
        for &item in &items {
            let n = self.arena.get(item).clone();
            match n.op {
                Operation::Case | Operation::Default => {
                    if decl_seen_after_case {
                        self.error_at(
                            CompileError::JumpingOverDeclarationStatementsCaseDisallowed,
                            &n,
                        )?;
                        decl_seen_after_case = false;
                    }
                }
                _ => {
                    if self.statement_introduces_declaration(item) {
                        decl_seen_after_case = true;
                    }
                }
            }
        }

        self.check_statement(node.right)?;
        self.switch_depth -= 1;
        Ok(())
    }

    fn statement_introduces_declaration(&self, node_id: NodeId) -> bool {
        if node_id == NULL_NODE { return false; }
        let n = self.arena.get(node_id);
        match n.op {
            Operation::KeywordDeclaration | Operation::ConstDeclaration => true,
            Operation::Statement | Operation::StatementNoDebug => {
                self.statement_introduces_declaration(n.left)
            }
            _ => false,
        }
    }

    fn check_switch_cases(
        &mut self,
        node_id: NodeId,
        seen_cases: &mut Vec<i32>,
        seen_default: &mut bool,
    ) -> Result<(), CompileError> {
        // Iterate the right-linked switch-body chain (recursing it overflows on
        // switches with ~500+ cases). Each chain link's `left` is a single
        // case/default/statement, so the per-link recursion is bounded.
        let mut chain = node_id;
        while chain != NULL_NODE {
            let link = self.arena.get(chain).clone();
            if link.op == Operation::StatementList {
                self.check_switch_cases(link.left, seen_cases, seen_default)?;
                chain = link.right;
                continue;
            }
            let node = link;

        match node.op {
            Operation::Case => {
                if node.left != NULL_NODE {
                    let val_node = self.arena.get(node.left).clone();
                    match val_node.op {
                        Operation::ConstantInteger => {
                            let v = val_node.int_data[0];
                            if seen_cases.contains(&v) {
                                self.error_at(
                                    CompileError::MultipleCaseConstantStatementsWithinSwitch,
                                    &node,
                                )?;
                            } else {
                                seen_cases.push(v);
                            }
                        }
                        Operation::ConstantString => {
                            // C++ accepts string case labels; their value is GetHash() of the literal.
                            let s = val_node.string_data.as_deref().unwrap_or("");
                            let v = crate::xxh32::cexo_string_hash(s);
                            if seen_cases.contains(&v) {
                                self.error_at(
                                    CompileError::MultipleCaseConstantStatementsWithinSwitch,
                                    &node,
                                )?;
                            } else {
                                seen_cases.push(v);
                            }
                        }
                        Operation::Variable => {
                            // C++ scriptcompfinalcode.cpp:1006-1027 only accepts a
                            // bare ConstantInteger / ConstantString in case labels.
                            // A Variable here only succeeds if it resolves to a known
                            // integer-valued const at fold time.
                            let name = val_node.string_data.as_deref().unwrap_or("");
                            let const_int = self.globals.iter()
                                .find(|v| v.name == name)
                                .and_then(|g| match &g.const_value {
                                    Some(DefaultValue::Integer(v)) => Some(*v),
                                    _ => None,
                                });
                            if let Some(v) = const_int {
                                if seen_cases.contains(&v) {
                                    self.error_at(
                                        CompileError::MultipleCaseConstantStatementsWithinSwitch,
                                        &node,
                                    )?;
                                } else {
                                    seen_cases.push(v);
                                }
                            } else {
                                self.error_at(
                                    CompileError::CaseParameterNotAConstantInteger,
                                    &node,
                                )?;
                            }
                        }
                        _ => {
                            self.error_at(
                                CompileError::CaseParameterNotAConstantInteger,
                                &node,
                            )?;
                        }
                    }
                }
            }
            Operation::Default => {
                if *seen_default {
                    self.error_at(
                        CompileError::MultipleDefaultStatementsWithinSwitch,
                        &node,
                    )?;
                }
                *seen_default = true;
            }
            _ => {}
        }
            // A non-list link is a single terminal item; stop after handling it.
            break;
        }
        Ok(())
    }

    fn check_expression(&mut self, node_id: NodeId) -> Result<NwType, CompileError> {
        if node_id == NULL_NODE {
            return Ok(NwType::Void);
        }
        // Guard against stack overflow on pathologically deep operator chains.
        self.expr_depth += 1;
        if self.expr_depth > 2000 {
            self.expr_depth -= 1;
            return Ok(NwType::Void);
        }
        let r = self.check_expression_inner(node_id);
        self.expr_depth -= 1;
        r
    }

    fn check_expression_inner(&mut self, node_id: NodeId) -> Result<NwType, CompileError> {
        let node = self.arena.get(node_id).clone();

        match node.op {
            Operation::ConstantInteger => Ok(NwType::Integer),
            Operation::ConstantFloat => Ok(NwType::Float),
            Operation::ConstantString => Ok(NwType::String),
            Operation::ConstantObject => Ok(NwType::Object),
            Operation::ConstantVector => Ok(NwType::Vector),
            Operation::ConstantJson => Ok(NwType::EngineStructure(7)),
            Operation::ConstantLocation => Ok(NwType::EngineStructure(2)),

            Operation::Variable => {
                let name = node.string_data.as_deref().unwrap_or("");
                if let Some(var) = self
                    .var_stack
                    .iter()
                    .rev()
                    .find(|v| v.name == name)
                {
                    return Ok(var.nw_type);
                }
                if let Some(g) = self.globals.iter().find(|v| v.name == name) {
                    return Ok(g.nw_type);
                }
                self.error_with_msg(
                    CompileError::UndefinedIdentifier,
                    &node,
                    name,
                )?;
                Ok(NwType::Void)
            }

            Operation::Action => {
                let action_id_node = node.left;
                if action_id_node == NULL_NODE {
                    return Ok(NwType::Void);
                }
                let aid = self.arena.get(action_id_node).clone();
                let func_name = aid.string_data.as_deref().unwrap_or("");

                let func = self.functions.iter().find(|f| f.name == func_name);
                if func.is_none() {
                    self.error_with_msg(
                        CompileError::UndefinedIdentifier,
                        &aid,
                        func_name,
                    )?;
                    return Ok(NwType::Void);
                }
                let func = func.unwrap().clone();
                // Note: C++ scriptcompfinalcode.cpp:914-924 raises UndefinedIdentifier
                // at link time for calls to user-declared-but-not-implemented
                // functions. Skipping that check here because the LSP frequently sees
                // forward declarations of stock NWN script functions whose actual
                // bodies live in BIF/KEY archives the LSP can't link against —
                // flagging them would drown real diagnostics in noise.

                // Check arguments
                let mut arg_node = aid.right;
                let mut arg_idx = 0;
                while arg_node != NULL_NODE {
                    let arg = self.arena.get(arg_node).clone();
                    if arg.left != NULL_NODE {
                        let arg_type = self.check_expression(arg.left)?;
                        if arg_idx < func.params.len() {
                            let expected = func.params[arg_idx].nw_type;
                            // C++ (scriptcompparsetree.cpp:818-822) only builds an
                            // ACTION_PARAMETER for a call to a VOID-returning function, so
                            // an `action` parameter accepts ONLY a void function call —
                            // not an int/float/string literal or a non-void call.
                            if expected == NwType::Action {
                                let arg_node = self.arena.get(arg.left).clone();
                                let is_void_call =
                                    arg_node.op == Operation::Action && arg_type == NwType::Void;
                                if !is_void_call {
                                    let cascade = self.diagnostics.iter().any(|d| d.line == arg_node.line);
                                    if !cascade {
                                        self.error_at(
                                            CompileError::DeclarationDoesNotMatchParameters,
                                            &arg_node,
                                        )?;
                                    }
                                }
                            } else if arg_type == NwType::Void
                                && expected != NwType::Void
                            {
                                self.error_at(
                                    CompileError::VoidExpressionWhereNonVoidRequired,
                                    &self.arena.get(arg.left).clone(),
                                )?;
                            } else if arg_type != expected
                                && arg_type != NwType::Void
                                && expected != NwType::Void
                                && expected != NwType::Action
                            {
                                self.error_at(
                                    CompileError::DeclarationDoesNotMatchParameters,
                                    &self.arena.get(arg.left).clone(),
                                )?;
                            } else if arg_type == NwType::Struct
                                && expected == NwType::Struct
                            {
                                // C++ scriptcompfinalcode.cpp:3355-3357: a struct argument
                                // must match the parameter's struct type-name, not just be
                                // "some struct".
                                let arg_name = self.resolve_struct_name(arg.left);
                                let param_name = func.params[arg_idx].type_name.clone();
                                if arg_name != param_name {
                                    self.error_at(
                                        CompileError::DeclarationDoesNotMatchParameters,
                                        &self.arena.get(arg.left).clone(),
                                    )?;
                                }
                            }
                        }
                    }
                    arg_idx += 1;
                    arg_node = arg.right;
                }

                // Check parameter count
                let min_params = func
                    .params
                    .iter()
                    .filter(|p| !p.has_default)
                    .count();
                if arg_idx < min_params || arg_idx > func.params.len() {
                    self.error_at(
                        CompileError::DeclarationDoesNotMatchParameters,
                        &aid,
                    )?;
                }

                Ok(func.return_type)
            }

            Operation::Assignment => {
                // C++ CheckForBadLValue (scriptcompparsetree.cpp:362-383) requires the LHS
                // to be an OPERATION_VARIABLE, or an OPERATION_STRUCTURE_PART whose base
                // chain terminates at a VARIABLE. `getFoo().x = 5` (base is an ACTION
                // call) is rejected with BAD_LVALUE (-575).
                if node.left != NULL_NODE {
                    let lhs = self.arena.get(node.left).clone();
                    let is_lvalue = match lhs.op {
                        Operation::Variable => true,
                        Operation::StructurePart => self.struct_part_roots_at_variable(node.left),
                        _ => false,
                    };
                    if !is_lvalue {
                        // C++ CheckForBadLValue raises BAD_LVALUE (-575) for an LHS not
                        // rooted at a variable (scriptcompparsetree.cpp:362-383).
                        self.error_at(CompileError::BadLValue, &node)?;
                    }
                    if lhs.op == Operation::Variable {
                        let name = lhs.string_data.as_deref().unwrap_or("");
                        let is_const = self.var_stack.iter().rev().find(|v| v.name == name)
                            .map(|v| v.is_constant)
                            .or_else(|| self.globals.iter().find(|v| v.name == name).map(|v| v.is_constant))
                            .unwrap_or(false);
                        if is_const {
                            self.error_at(CompileError::InvalidValueAssignedToConstant, &node)?;
                        }
                    }
                }

                let left_type = self.check_expression(node.left)?;
                let right_type = self.check_expression(node.right)?;

                let op_token = node.int_data[0];
                let is_compound = op_token != crate::token::TokenType::AssignmentEqual as i32
                    && op_token != 0;

                if is_compound && left_type != NwType::Void && right_type != NwType::Void {
                    // C++: a += b desugars to a = a + b — check binop validity & result type
                    use crate::token::TokenType as T;
                    let is_plus = op_token == T::AssignmentPlus as i32;
                    let arith = is_plus
                        || op_token == T::AssignmentMinus as i32
                        || op_token == T::AssignmentMultiply as i32
                        || op_token == T::AssignmentDivide as i32
                        || op_token == T::AssignmentModulus as i32;
                    let bitwise = op_token == T::AssignmentAnd as i32
                        || op_token == T::AssignmentOr as i32
                        || op_token == T::AssignmentXor as i32;
                    let shift = op_token == T::AssignmentShiftLeft as i32
                        || op_token == T::AssignmentShiftRight as i32
                        || op_token == T::AssignmentUShiftRight as i32;

                    if arith {
                        // C++ desugars `x OP= y` to `x = OP(x, y)` and type-checks the
                        // inner OP first. Use the SAME per-operator operand table as the
                        // non-compound path (auxcode_pair is too permissive — it accepts
                        // e.g. vector+float and float%float which the operator rules reject
                        // with -592, before the assignment-level -618 is ever reached).
                        let inner_op = if is_plus {
                            Operation::Add
                        } else if op_token == T::AssignmentMinus as i32 {
                            Operation::Subtract
                        } else if op_token == T::AssignmentMultiply as i32 {
                            Operation::Multiply
                        } else if op_token == T::AssignmentDivide as i32 {
                            Operation::Divide
                        } else {
                            Operation::Modulus
                        };
                        let valid = match (left_type, right_type) {
                            (NwType::Integer, NwType::Integer) => true,
                            (NwType::Float, NwType::Float) => !matches!(inner_op, Operation::Modulus),
                            (NwType::Integer, NwType::Float) | (NwType::Float, NwType::Integer) => {
                                !matches!(inner_op, Operation::Modulus)
                            }
                            (NwType::Vector, NwType::Vector) => {
                                matches!(inner_op, Operation::Add | Operation::Subtract)
                            }
                            (NwType::Vector, NwType::Float) => {
                                matches!(inner_op, Operation::Multiply | Operation::Divide)
                            }
                            (NwType::Float, NwType::Vector) => matches!(inner_op, Operation::Multiply),
                            (NwType::String, NwType::String) => matches!(inner_op, Operation::Add),
                            _ => false,
                        };
                        if !valid {
                            self.error_at(CompileError::ArithmeticOperationHasInvalidOperands, &node)?;
                        } else {
                            // The OP result type must match the assignment target.
                            let result_type = if left_type == NwType::Vector || right_type == NwType::Vector {
                                NwType::Vector
                            } else if left_type == NwType::Float || right_type == NwType::Float {
                                NwType::Float
                            } else if left_type == NwType::String && is_plus {
                                NwType::String
                            } else {
                                NwType::Integer
                            };
                            if result_type != left_type {
                                self.error_at(CompileError::MismatchedTypes, &node)?;
                            }
                        }
                    } else if bitwise {
                        if left_type != NwType::Integer || right_type != NwType::Integer {
                            self.error_at(CompileError::LogicalOperationHasInvalidOperands, &node)?;
                        }
                    } else if shift {
                        if left_type != NwType::Integer || right_type != NwType::Integer {
                            self.error_at(CompileError::ShiftOperationHasInvalidOperands, &node)?;
                        }
                    }
                } else if left_type != NwType::Void {
                    let types_match = if left_type == NwType::Struct && right_type == NwType::Struct {
                        self.resolve_struct_name(node.left) == self.resolve_struct_name(node.right)
                    } else {
                        left_type == right_type
                    };
                    if !types_match {
                        self.error_at(CompileError::MismatchedTypes, &node)?;
                    }
                }
                Ok(left_type)
            }

            Operation::Add | Operation::Subtract | Operation::Multiply
            | Operation::Divide | Operation::Modulus => {
                let left_type = self.check_expression(node.left)?;
                let right_type = self.check_expression(node.right)?;

                // Per C++ rules (scriptcompfinalcode.cpp): only specific operand-type
                // combinations are valid for each arithmetic operator.
                let op = node.op;
                let valid = match (left_type, right_type) {
                    (NwType::Integer, NwType::Integer) => true,
                    (NwType::Float, NwType::Float) => !matches!(op, Operation::Modulus),
                    (NwType::Integer, NwType::Float) | (NwType::Float, NwType::Integer) => {
                        !matches!(op, Operation::Modulus)
                    }
                    (NwType::Vector, NwType::Vector) => {
                        matches!(op, Operation::Add | Operation::Subtract)
                    }
                    (NwType::Vector, NwType::Float) => {
                        matches!(op, Operation::Multiply | Operation::Divide)
                    }
                    (NwType::Float, NwType::Vector) => matches!(op, Operation::Multiply),
                    (NwType::String, NwType::String) => matches!(op, Operation::Add),
                    _ => false,
                };
                if !valid {
                    self.error_at(
                        CompileError::ArithmeticOperationHasInvalidOperands,
                        &node,
                    )?;
                }

                if left_type == NwType::Vector || right_type == NwType::Vector {
                    Ok(NwType::Vector)
                } else if left_type == NwType::Float || right_type == NwType::Float {
                    Ok(NwType::Float)
                } else if left_type == NwType::String && node.op == Operation::Add {
                    Ok(NwType::String)
                } else {
                    Ok(NwType::Integer)
                }
            }

            Operation::Negation => {
                // Unary - requires int or float per C++
                let t = self.check_expression(node.left)?;
                if t != NwType::Void && t != NwType::Integer && t != NwType::Float {
                    self.error_at(CompileError::ArithmeticOperationHasInvalidOperands, &node)?;
                }
                Ok(t)
            }

            Operation::BooleanNot | Operation::OnesComplement => {
                // ! and ~ require an integer operand. C++ scriptcompfinalcode.cpp:4685,4710
                // reports ARITHMETIC_OPERATION_HAS_INVALID_OPERANDS (-592) here — the
                // -584 "non-integer where integer required" code is reserved for
                // if/while/ternary condition contexts.
                let t = self.check_expression(node.left)?;
                // ! / ~ require an integer operand; a void operand (e.g. void call
                // result) is rejected by C++ here too. Suppress cascade on already-
                // reported operands.
                if t != NwType::Integer {
                    let cascade = t == NwType::Void
                        && self.diagnostics.iter().any(|d| d.line == node.line);
                    if !cascade {
                        self.error_at(CompileError::ArithmeticOperationHasInvalidOperands, &node)?;
                    }
                }
                Ok(NwType::Integer)
            }

            Operation::LogicalAnd | Operation::LogicalOr => {
                let lt = self.check_expression(node.left)?;
                let rt = self.check_expression(node.right)?;
                self.require_integer_operands(lt, rt, CompileError::LogicalOperationHasInvalidOperands, &node)?;
                Ok(NwType::Integer)
            }

            Operation::InclusiveOr | Operation::ExclusiveOr | Operation::BooleanAnd => {
                let lt = self.check_expression(node.left)?;
                let rt = self.check_expression(node.right)?;
                self.require_integer_operands(lt, rt, CompileError::LogicalOperationHasInvalidOperands, &node)?;
                Ok(NwType::Integer)
            }

            Operation::ShiftLeft | Operation::ShiftRight | Operation::UnsignedShiftRight => {
                let lt = self.check_expression(node.left)?;
                let rt = self.check_expression(node.right)?;
                self.require_integer_operands(lt, rt, CompileError::ShiftOperationHasInvalidOperands, &node)?;
                Ok(NwType::Integer)
            }

            Operation::ConditionEqual | Operation::ConditionNotEqual
            | Operation::ConditionGEQ | Operation::ConditionGT
            | Operation::ConditionLT | Operation::ConditionLEQ => {
                let lt = self.check_expression(node.left)?;
                let rt = self.check_expression(node.right)?;

                // Validate operand types — match C++ strictly (no int↔float promotion).
                let is_equality = matches!(
                    node.op,
                    Operation::ConditionEqual | Operation::ConditionNotEqual
                );
                let err = if is_equality {
                    CompileError::EqualityTestHasInvalidOperands
                } else {
                    CompileError::ComparisonTestHasInvalidOperands
                };
                if lt == NwType::Void || rt == NwType::Void {
                    // C++ rejects a void operand here too; suppress cascade noise.
                    let cascade = self.diagnostics.iter().any(|d| d.line == node.line);
                    if !cascade {
                        self.error_at(err, &node)?;
                    }
                } else if is_equality {
                    // Equality: types must match exactly. For structs, names must match.
                    let ok = if lt == NwType::Struct && rt == NwType::Struct {
                        self.resolve_struct_name(node.left) == self.resolve_struct_name(node.right)
                    } else {
                        lt == rt
                    };
                    if !ok {
                        self.error_at(err, &node)?;
                    }
                } else {
                    // Ordering (<, >, <=, >=): only (int,int) or (float,float)
                    let ok = (lt == NwType::Integer && rt == NwType::Integer)
                        || (lt == NwType::Float && rt == NwType::Float);
                    if !ok {
                        self.error_at(err, &node)?;
                    }
                }
                Ok(NwType::Integer)
            }

            Operation::PostIncrement | Operation::PostDecrement
            | Operation::PreIncrement | Operation::PreDecrement => {
                let t = self.check_expression(node.left)?;
                // C++: ++/-- requires integer lvalue
                if t != NwType::Void && t != NwType::Integer {
                    self.error_at(CompileError::OperandMustBeAnIntegerLValue, &node)?;
                }
                // Also require an lvalue (variable or struct field)
                if node.left != NULL_NODE {
                    let lhs = self.arena.get(node.left).clone();
                    if !matches!(lhs.op, Operation::Variable | Operation::StructurePart) {
                        self.error_at(CompileError::OperandMustBeAnIntegerLValue, &node)?;
                    }
                    // ++/-- mutate the operand, so the same const-mutation rule that
                    // applies to `=` and `+=` applies here (consistent with the project's
                    // adopted const enforcement).
                    if lhs.op == Operation::Variable {
                        let name = lhs.string_data.as_deref().unwrap_or("");
                        let is_const = self.var_stack.iter().rev().find(|v| v.name == name)
                            .map(|v| v.is_constant)
                            .or_else(|| self.globals.iter().find(|v| v.name == name).map(|v| v.is_constant))
                            .unwrap_or(false);
                        if is_const {
                            self.error_at(CompileError::InvalidValueAssignedToConstant, &node)?;
                        }
                    }
                }
                Ok(NwType::Integer)
            }

            Operation::StructurePart => {
                let struct_type = self.check_expression(node.left)?;
                let field_name = node.string_data.as_deref().unwrap_or("");

                if struct_type == NwType::Vector {
                    match field_name {
                        "x" | "y" | "z" => return Ok(NwType::Float),
                        _ => {
                            self.error_at(CompileError::UndefinedFieldInStructure, &node)?;
                            return Ok(NwType::Void);
                        }
                    }
                }

                if struct_type == NwType::Struct {
                    // Find the struct type name from the left-hand expression
                    let struct_name = self.resolve_struct_name(node.left);
                    if let Some(ref sn) = struct_name {
                        if let Some(sd) = self.structs.iter().find(|s| s.name == *sn) {
                            // Struct is known — check the field exists
                            if let Some(field) = sd.fields.iter().find(|f| f.name == field_name) {
                                return Ok(field.nw_type);
                            }
                            // Struct known but field doesn't exist — real error
                            self.error_at(CompileError::UndefinedFieldInStructure, &node)?;
                            return Ok(NwType::Void);
                        }
                    }
                    // Struct definition unknown (probably from an unloaded include) — silent
                    return Ok(NwType::Void);
                }

                // C++ rejects `.` on non-struct/non-vector types.
                // Allow Void silently (means upstream already errored).
                if struct_type != NwType::Void {
                    self.error_at(CompileError::LeftOfStructurePartNotStructure, &node)?;
                }
                Ok(NwType::Void)
            }

            Operation::CondBlock => {
                // Ternary: cond ? then : else
                if node.left != NULL_NODE {
                    let cond = self.arena.get(node.left).clone();
                    // Unlike if/while/do, C++ does NOT wrap the ternary condition in an
                    // INTEGER_EXPRESSION node — its grammar child is a bare
                    // LOGICAL_OR_EXPRESSION (scriptcompparsetree.cpp:1686), and
                    // COND_CONDITION handling (scriptcompfinalcode.cpp:3547-3566) only
                    // enforces integer for SWITCH_CONDITION. So a float/string/object
                    // ternary condition is valid; only check the sub-expression itself.
                    self.check_expression(cond.left)?;
                }
                if node.right != NULL_NODE {
                    let choice = self.arena.get(node.right).clone();
                    let then_type = self.check_expression(choice.left)?;
                    let else_type = self.check_expression(choice.right)?;
                    // C++ scriptcompfinalcode.cpp:3839-3843: types must match, and for
                    // struct branches the struct NAMES must match too.
                    let mismatch = if then_type == NwType::Struct && else_type == NwType::Struct {
                        self.resolve_struct_name(choice.left) != self.resolve_struct_name(choice.right)
                    } else {
                        then_type != else_type
                    };
                    if mismatch {
                        self.error_at(
                            CompileError::ConditionalMustHaveMatchingReturnTypes,
                            &choice,
                        )?;
                    }
                    return Ok(then_type);
                }
                Ok(NwType::Void)
            }

            Operation::NonVoidExpression | Operation::IntegerExpression => {
                self.check_expression(node.left)
            }

            Operation::ActionArgList => {
                self.check_expression(node.left)
            }

            // The for-loop UPDATE clause is wrapped in a WhileContinue node. C++ walks
            // it like any other expression (scriptcompfinalcode.cpp:5798), so it must be
            // type-checked too — otherwise `for(...; ...; i = 3.5)` is silently accepted.
            Operation::WhileContinue => {
                self.check_expression(node.left)
            }

            Operation::Case | Operation::Default => Ok(NwType::Void),

            _ => Ok(NwType::Void),
        }
    }

    /// Capture a literal parameter-default value from the AST so it can be emitted
    /// verbatim at call sites where the trailing argument is omitted.
    fn extract_default_value(arena: &AstArena, node_id: NodeId) -> Option<DefaultValue> {
        Self::extract_default_value_with_globals(arena, node_id, None)
    }

    fn extract_default_value_with_globals(
        arena: &AstArena,
        node_id: NodeId,
        globals: Option<&[VarEntry]>,
    ) -> Option<DefaultValue> {
        if node_id == NULL_NODE { return None; }
        let n = arena.get(node_id);
        match n.op {
            Operation::ConstantInteger => Some(DefaultValue::Integer(n.int_data[0])),
            Operation::ConstantFloat => Some(DefaultValue::Float(n.float_data)),
            Operation::ConstantString => Some(DefaultValue::String(
                n.string_data.clone().unwrap_or_default(),
            )),
            Operation::ConstantObject => Some(DefaultValue::Object(n.int_data[0])),
            Operation::ConstantVector => {
                let arg1 = if n.left != NULL_NODE { Some(arena.get(n.left)) } else { None };
                let arg2 = arg1.and_then(|a| if a.right != NULL_NODE { Some(arena.get(a.right)) } else { None });
                let arg3 = arg2.and_then(|a| if a.right != NULL_NODE { Some(arena.get(a.right)) } else { None });
                let get_f = |a: Option<&AstNode>| -> Option<f32> {
                    let a = a?;
                    if a.left == NULL_NODE { return None; }
                    let lit = arena.get(a.left);
                    if lit.op == Operation::ConstantFloat { Some(lit.float_data) } else { None }
                };
                Some(DefaultValue::Vector(
                    get_f(arg1).unwrap_or(0.0),
                    get_f(arg2).unwrap_or(0.0),
                    get_f(arg3).unwrap_or(0.0),
                ))
            }
            Operation::Negation => {
                let inner = if n.left != NULL_NODE { arena.get(n.left) } else { return None };
                match inner.op {
                    Operation::ConstantInteger => Some(DefaultValue::Integer(inner.int_data[0].wrapping_neg())),
                    Operation::ConstantFloat => Some(DefaultValue::Float(-inner.float_data)),
                    _ => None,
                }
            }
            Operation::ConstantJson | Operation::ConstantLocation => Some(DefaultValue::EngineStruct),
            Operation::Variable => {
                // Engine constants like OBJECT_INVALID, TRUE, FALSE are globals
                // whose initializers are constant. Look up the initializer and
                // capture its literal value.
                let name = n.string_data.as_deref()?;
                let globals = globals?;
                let entry = globals.iter().find(|v| v.name == name)?;
                entry.const_value.clone()
            }
            _ => None,
        }
    }

    fn is_constant_expression(&self, node_id: NodeId) -> bool {
        if node_id == NULL_NODE { return true; }
        let node = self.arena.get(node_id);
        match node.op {
            Operation::ConstantInteger
            | Operation::ConstantFloat
            | Operation::ConstantString
            | Operation::ConstantObject
            | Operation::ConstantJson
            | Operation::ConstantLocation => true,
            Operation::ConstantVector => {
                self.is_constant_expression(node.left) && self.is_constant_expression(node.right)
            }
            Operation::Negation | Operation::BooleanNot | Operation::OnesComplement => {
                self.is_constant_expression(node.left)
            }
            Operation::Variable => {
                // Engine constants like TRUE/FALSE/OBJECT_INVALID are declared as
                // plain globals in nwscript.nss (without `const`), but the C++ compiler
                // accepts any global identifier reference here and reads its initializer
                // at codegen time. Accept any known global to match that behavior.
                let name = match node.string_data.as_deref() { Some(s) => s, None => return false };
                self.globals.iter().any(|v| v.name == name)
            }
            _ => false,
        }
    }

    /// Does a `.field` (StructurePart) chain bottom out at a plain Variable?
    /// `s.a.b` → yes (root is Variable `s`); `getFoo().x` → no (root is a call).
    fn struct_part_roots_at_variable(&self, node_id: NodeId) -> bool {
        let mut cur = node_id;
        loop {
            if cur == NULL_NODE { return false; }
            let n = self.arena.get(cur);
            match n.op {
                Operation::Variable => return true,
                Operation::StructurePart => cur = n.left,
                _ => return false,
            }
        }
    }

    fn resolve_struct_name(&self, node_id: NodeId) -> Option<String> {
        if node_id == NULL_NODE { return None; }
        let node = self.arena.get(node_id);

        match node.op {
            Operation::Variable => {
                let name = node.string_data.as_deref()?;
                // Check locals
                if let Some(var) = self.var_stack.iter().rev().find(|v| v.name == name) {
                    return var.type_name.clone();
                }
                // Check globals
                if let Some(g) = self.globals.iter().find(|v| v.name == name) {
                    return g.type_name.clone();
                }
                None
            }
            Operation::StructurePart => {
                // Chained access: e.vParam0.x — resolve the left side's struct,
                // find the field type, and return its type name
                let parent_struct = self.resolve_struct_name(node.left)?;
                let field_name = node.string_data.as_deref()?;
                let sd = self.structs.iter().find(|s| s.name == parent_struct)?;
                let field = sd.fields.iter().find(|f| f.name == field_name)?;
                field.type_name.clone()
            }
            Operation::Action => {
                // Function call return type
                if node.left == NULL_NODE { return None; }
                let aid = self.arena.get(node.left);
                let func_name = aid.string_data.as_deref()?;
                let func = self.functions.iter().find(|f| f.name == func_name)?;
                func.return_type_name.clone()
            }
            Operation::CondBlock => {
                // Ternary (`cond ? a : b`): C++ propagates the branch struct name onto
                // the ternary node (scriptcompfinalcode.cpp:3839-3874). semcheck holds an
                // immutable arena, so resolve it on demand from the then-branch (the two
                // branches already had their struct names compared when checked).
                // CondBlock.right is the CondChoice; its .left is the then-branch.
                if node.right == NULL_NODE { return None; }
                let choice_left = self.arena.get(node.right).left;
                self.resolve_struct_name(choice_left)
            }
            _ => node.type_name.clone(),
        }
    }

    /// Map of every resolved `const` name to its literal value (main file + includes,
    /// including const-from-const). Codegen uses this to fold a const reference to its
    /// literal at the use site, matching C++ (consts allocate no runtime storage).
    pub fn collect_const_values(&self) -> std::collections::HashMap<String, DefaultValue> {
        let mut m = std::collections::HashMap::new();
        for g in &self.globals {
            if g.is_constant {
                if let Some(dv) = &g.const_value {
                    m.insert(g.name.clone(), dv.clone());
                }
            }
        }
        m
    }

    pub fn find_function(&self, name: &str) -> Option<&FunctionSig> {
        self.functions.iter().find(|f| f.name == name)
    }

    pub fn find_struct(&self, name: &str) -> Option<&StructDef> {
        self.structs.iter().find(|s| s.name == name)
    }
}

fn op_to_nw_type(op: Operation) -> NwType {
    match op {
        Operation::KeywordInt => NwType::Integer,
        Operation::KeywordFloat => NwType::Float,
        Operation::KeywordString => NwType::String,
        Operation::KeywordObject => NwType::Object,
        Operation::KeywordVoid => NwType::Void,
        Operation::KeywordVector => NwType::Vector,
        Operation::KeywordStruct => NwType::Struct,
        Operation::KeywordEngineStructure(n) => NwType::EngineStructure(n),
        _ => NwType::Void,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn check(src: &str) -> SemanticChecker<'static> {
        check_with_options(src, false, true)
    }

    fn check_with_options(
        src: &str,
        collect_all: bool,
        require_entry: bool,
    ) -> SemanticChecker<'static> {
        let src = Box::leak(src.to_string().into_boxed_str());
        let mut lexer = Lexer::new(src, "test.nss", 0);
        let tokens = lexer.tokenize();
        let mut parser = Parser::new(tokens);
        parser.file_names.push("test.nss".to_string());
        let root = parser.parse_program().unwrap();

        let arena = Box::leak(Box::new(std::mem::replace(&mut parser.arena, AstArena::new())));
        let file_names: &'static [String] =
            Box::leak(parser.file_names.into_boxed_slice());

        let mut checker = SemanticChecker::new(arena, file_names);
        checker.set_collect_all_errors(collect_all);
        checker.set_require_entry_point(require_entry);
        let _ = checker.check(root);
        checker
    }

    #[test]
    fn test_valid_program() {
        let c = check_with_options("void main() { int x = 1 + 2; }", false, true);
        assert!(c.diagnostics.is_empty(), "{:?}", c.diagnostics);
    }

    #[test]
    fn test_missing_main() {
        let c = check_with_options("int helper(int n) { return n; }", false, true);
        assert!(!c.diagnostics.is_empty());
        assert_eq!(
            c.diagnostics[0].error,
            CompileError::NoFunctionMainInScript
        );
    }

    #[test]
    fn test_no_entry_point_required() {
        let c = check_with_options("int helper(int n) { return n; }", false, false);
        assert!(c.diagnostics.is_empty(), "{:?}", c.diagnostics);
    }

    #[test]
    fn test_type_mismatch() {
        let c = check_with_options(
            "void main() { string s = 42; }",
            true,
            true,
        );
        assert!(!c.diagnostics.is_empty());
        assert!(c.diagnostics.iter().any(|d| d.error == CompileError::MismatchedTypes));
    }

    #[test]
    fn test_undefined_identifier() {
        let c = check_with_options(
            "void main() { DoesNotExist(); }",
            true,
            true,
        );
        assert!(!c.diagnostics.is_empty());
        assert!(c.diagnostics.iter().any(|d| d.error == CompileError::UndefinedIdentifier));
    }

    #[test]
    fn test_multi_error_collection() {
        let c = check_with_options(
            r#"
            void main() { string s = 42; }
            void other() { int i = "hello"; }
            void third() { int j = "x"; }
            "#,
            true,
            false,
        );
        let type_errors: Vec<_> = c
            .diagnostics
            .iter()
            .filter(|d| d.error == CompileError::MismatchedTypes)
            .collect();
        assert_eq!(type_errors.len(), 3, "Expected 3 type errors, got {:?}", c.diagnostics);
    }

    #[test]
    fn test_function_declaration_and_call() {
        let c = check_with_options(
            r#"
            int add(int a, int b);
            int add(int a, int b) { return a + b; }
            void main() { int x = add(1, 2); }
            "#,
            false,
            true,
        );
        assert!(c.diagnostics.is_empty(), "{:?}", c.diagnostics);
    }

    #[test]
    fn test_variable_scope() {
        let c = check_with_options(
            r#"
            void main() {
                int x = 1;
                if (x > 0) {
                    int y = 2;
                }
            }
            "#,
            false,
            true,
        );
        assert!(c.diagnostics.is_empty(), "{:?}", c.diagnostics);
    }

    #[test]
    fn test_struct_definition() {
        let c = check_with_options(
            r#"
            struct Vec2 { int x; int y; };
            void main() { }
            "#,
            false,
            true,
        );
        assert!(c.diagnostics.is_empty(), "{:?}", c.diagnostics);
        // The checker pre-seeds a "vector" pseudo-struct, so user structs follow.
        let vec2 = c.structs.iter().find(|s| s.name == "Vec2").expect("Vec2 registered");
        assert_eq!(vec2.fields.len(), 2);
    }

    #[test]
    fn test_two_undefined_in_two_functions() {
        let c = check_with_options(
            r#"
            void main() { DoesNotExistA(); }
            void other() { DoesNotExistB(); }
            "#,
            true,
            false,
        );
        let undef_errors: Vec<_> = c
            .diagnostics
            .iter()
            .filter(|d| d.error == CompileError::UndefinedIdentifier)
            .collect();
        assert_eq!(undef_errors.len(), 2, "{:?}", c.diagnostics);
        assert!(undef_errors[0].message.contains("DoesNotExistA"));
        assert!(undef_errors[1].message.contains("DoesNotExistB"));
    }

    #[test]
    fn test_switch_requires_integer() {
        let c = check_with_options(
            r#"
            void main() {
                string s = "hello";
                switch (s) {
                    default: break;
                }
            }
            "#,
            true,
            true,
        );
        assert!(c.diagnostics.iter().any(|d| d.error == CompileError::SwitchMustEvaluateToAnInteger));
    }
}
