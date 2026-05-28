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
pub struct ParamInfo {
    pub name: String,
    pub nw_type: NwType,
    pub type_name: Option<String>,
    pub has_default: bool,
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
    collect_all_errors: bool,
    require_entry_point: bool,
    loop_depth: u32,
    switch_depth: u32,
}

impl<'a> SemanticChecker<'a> {
    pub fn new(arena: &'a AstArena, file_names: &'a [String]) -> Self {
        Self {
            arena,
            file_names,
            diagnostics: Vec::new(),
            functions: Vec::new(),
            structs: Vec::new(),
            globals: Vec::new(),
            var_stack: Vec::new(),
            scope_level: 0,
            current_function: None,
            current_return_type: NwType::Void,
            collect_all_errors: false,
            require_entry_point: true,
            loop_depth: 0,
            switch_depth: 0,
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
                    self.globals.push(VarEntry {
                        name,
                        nw_type: node.nw_type,
                        type_name: node.type_name.clone(),
                        scope_level: 0,
                        is_constant: true,
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

                // Detect recursive struct: field type same as containing struct
                if var.nw_type == NwType::Struct {
                    if let Some(tn) = &var.type_name {
                        if *tn == name {
                            let _ = self.error_at(CompileError::UndefinedStructure, &var);
                        }
                    }
                }

                let size = self.field_size(var.nw_type, &var.type_name);
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
                self.globals.push(VarEntry {
                    name: var_name,
                    nw_type,
                    type_name: type_name.clone(),
                    scope_level: 0,
                    is_constant: false,
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
                params.push(ParamInfo {
                    name: p.string_data.as_deref().unwrap_or("").to_string(),
                    nw_type: p.nw_type,
                    type_name: p.type_name.clone(),
                    has_default: p.left != NULL_NODE,
                });
            }
            param_node = p.right;
        }

        if let Some(existing) = self.functions.iter_mut().find(|f| f.name == name) {
            if has_impl {
                existing.has_implementation = true;
            }
        } else {
            self.functions.push(FunctionSig {
                name,
                return_type: fid.nw_type,
                return_type_name: fid.type_name.clone(),
                params,
                has_implementation: has_impl,
                is_engine_action: is_engine,
                action_id: 0,
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
            let has_main = self.functions.iter().any(|f| {
                f.name == "main"
                    && f.return_type == NwType::Void
                    && f.params.is_empty()
                    && f.has_implementation
            });
            let has_intsc = self.functions.iter().any(|f| {
                f.name == "StartingConditional"
                    && f.return_type == NwType::Integer
                    && f.params.is_empty()
                    && f.has_implementation
            });
            if !has_main && !has_intsc {
                let node = self.arena.get(root);
                self.error_at(CompileError::NoFunctionMainInScript, node)?;
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

        let mut fields = Vec::new();
        let mut offset = 0;
        let mut field_chain = def_node.left;

        while field_chain != NULL_NODE {
            let vl = self.arena.get(field_chain).clone();
            if vl.left != NULL_NODE {
                let var = self.arena.get(vl.left).clone();
                let field_name = var.string_data.as_deref().unwrap_or("").to_string();

                // Detect recursive struct: field type same as containing struct
                if var.nw_type == NwType::Struct {
                    if let Some(tn) = &var.type_name {
                        if *tn == name {
                            let _ = self.error_at(CompileError::UndefinedStructure, &var);
                        }
                    }
                }

                let size = self.field_size(var.nw_type, &var.type_name);
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
                params.push(ParamInfo {
                    name: pname,
                    nw_type: p.nw_type,
                    type_name: p.type_name.clone(),
                    has_default: p.left != NULL_NODE,
                });
            }
            param_node = p.right;
        }

        let new_return = fid.nw_type;
        let new_return_name = fid.type_name.clone();

        let existing_idx = self.functions.iter().position(|f| f.name == name);
        if let Some(idx) = existing_idx {
            let return_matches = self.functions[idx].return_type == new_return
                && self.functions[idx].return_type_name == new_return_name;
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
            if has_impl {
                if already_implemented {
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
                }

                self.globals.push(VarEntry {
                    name: var_name,
                    nw_type,
                    type_name: type_name.clone(),
                    scope_level: 0,
                    is_constant: false,
                });
            }
            vl_id = vl.right;
        }
    }

    fn register_const_global(&mut self, node: &AstNode) {
        let name = node.string_data.as_deref().unwrap_or("").to_string();
        let declared_type = node.nw_type;

        // Check initializer type matches declared type
        if node.left != NULL_NODE {
            let init_type = match self.infer_const_type(node.left) {
                Some(t) => t,
                None => declared_type, // can't infer — don't error
            };
            if init_type != declared_type
                && init_type != NwType::Void
                && declared_type != NwType::Void
            {
                let _ = self.error_at(CompileError::MismatchedTypes, node);
            }
        }

        self.globals.push(VarEntry {
            name,
            nw_type: declared_type,
            type_name: node.type_name.clone(),
            scope_level: 0,
            is_constant: true,
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
                // If any child returns, the chain returns
                self.all_paths_return(node.left) || self.all_paths_return(node.right)
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
            Operation::SwitchBlock => {
                // Conservatively: only if all cases return AND there's a default
                self.switch_all_paths_return(node.right)
            }
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
        if node_id == NULL_NODE { return; }
        let node = self.arena.get(node_id);
        if node.op == Operation::StatementList {
            self.flatten_stmt_list(node.left, out);
            self.flatten_stmt_list(node.right, out);
        } else {
            out.push(node_id);
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
                self.check_statement(node.left)?;
                self.check_statement(node.right)?;
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
                    } else if ret_type != self.current_return_type
                        && ret_type != NwType::Void
                    {
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
                    if init_type != decl_type
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
                });
            }
            vl_id = vl.right;
        }

        Ok(())
    }

    fn check_if(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        // IfCondition
        if node.left != NULL_NODE {
            let cond = self.arena.get(node.left).clone();
            if cond.left != NULL_NODE {
                self.check_expression(cond.left)?;
            }
        }
        // IfChoice (left = then, right = else)
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
                self.check_expression(cond.left)?;
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

    fn check_do_while(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        self.loop_depth += 1;
        self.check_statement(node.left)?;
        self.loop_depth -= 1;
        if node.right != NULL_NODE {
            let cond = self.arena.get(node.right).clone();
            if cond.left != NULL_NODE {
                self.check_expression(cond.left)?;
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

        self.check_statement(node.right)?;
        self.switch_depth -= 1;
        Ok(())
    }

    fn check_switch_cases(
        &mut self,
        node_id: NodeId,
        seen_cases: &mut Vec<i32>,
        seen_default: &mut bool,
    ) -> Result<(), CompileError> {
        if node_id == NULL_NODE { return Ok(()); }
        let node = self.arena.get(node_id).clone();

        if node.op == Operation::StatementList {
            self.check_switch_cases(node.left, seen_cases, seen_default)?;
            self.check_switch_cases(node.right, seen_cases, seen_default)?;
            return Ok(());
        }

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
                        Operation::Variable | Operation::Negation => {
                            // Const global reference or negated literal — accept,
                            // can't track duplicate value without const folding.
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
        Ok(())
    }

    fn check_expression(&mut self, node_id: NodeId) -> Result<NwType, CompileError> {
        if node_id == NULL_NODE {
            return Ok(NwType::Void);
        }
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

                // Check arguments
                let mut arg_node = aid.right;
                let mut arg_idx = 0;
                while arg_node != NULL_NODE {
                    let arg = self.arena.get(arg_node).clone();
                    if arg.left != NULL_NODE {
                        let arg_type = self.check_expression(arg.left)?;
                        if arg_idx < func.params.len() {
                            let expected = func.params[arg_idx].nw_type;
                            if arg_type != expected
                                && arg_type != NwType::Void
                                && expected != NwType::Void
                            {
                                self.error_at(
                                    CompileError::MismatchedTypes,
                                    &self.arena.get(arg.left).clone(),
                                )?;
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
                // Check if assigning to a const
                if node.left != NULL_NODE {
                    let lhs = self.arena.get(node.left).clone();
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
                if left_type != right_type
                    && left_type != NwType::Void
                    && right_type != NwType::Void
                {
                    self.error_at(CompileError::MismatchedTypes, &node)?;
                }
                Ok(left_type)
            }

            Operation::Add | Operation::Subtract | Operation::Multiply
            | Operation::Divide | Operation::Modulus => {
                let left_type = self.check_expression(node.left)?;
                let right_type = self.check_expression(node.right)?;

                if left_type.auxcode_pair(right_type).is_none() {
                    self.error_at(
                        CompileError::ArithmeticOperationHasInvalidOperands,
                        &node,
                    )?;
                }

                // Result type depends on operand types
                if left_type == NwType::Float || right_type == NwType::Float {
                    Ok(NwType::Float)
                } else if left_type == NwType::String && node.op == Operation::Add {
                    Ok(NwType::String)
                } else if left_type == NwType::Vector {
                    Ok(NwType::Vector)
                } else {
                    Ok(NwType::Integer)
                }
            }

            Operation::Negation => {
                let t = self.check_expression(node.left)?;
                Ok(t)
            }

            Operation::BooleanNot | Operation::OnesComplement => {
                self.check_expression(node.left)?;
                Ok(NwType::Integer)
            }

            Operation::LogicalAnd | Operation::LogicalOr => {
                self.check_expression(node.left)?;
                self.check_expression(node.right)?;
                Ok(NwType::Integer)
            }

            Operation::InclusiveOr | Operation::ExclusiveOr | Operation::BooleanAnd
            | Operation::ShiftLeft | Operation::ShiftRight | Operation::UnsignedShiftRight => {
                self.check_expression(node.left)?;
                self.check_expression(node.right)?;
                Ok(NwType::Integer)
            }

            Operation::ConditionEqual | Operation::ConditionNotEqual
            | Operation::ConditionGEQ | Operation::ConditionGT
            | Operation::ConditionLT | Operation::ConditionLEQ => {
                let lt = self.check_expression(node.left)?;
                let rt = self.check_expression(node.right)?;

                // Validate operand types
                if lt != NwType::Void && rt != NwType::Void {
                    let is_equality = matches!(
                        node.op,
                        Operation::ConditionEqual | Operation::ConditionNotEqual
                    );
                    // Equality: types must match exactly (or int<->float promotion)
                    // Ordering: only int/float allowed
                    let compatible = lt == rt
                        || (matches!(lt, NwType::Integer | NwType::Float)
                            && matches!(rt, NwType::Integer | NwType::Float));
                    if !compatible {
                        let err = if is_equality {
                            CompileError::EqualityTestHasInvalidOperands
                        } else {
                            CompileError::ComparisonTestHasInvalidOperands
                        };
                        self.error_at(err, &node)?;
                    }
                }
                Ok(NwType::Integer)
            }

            Operation::PostIncrement | Operation::PostDecrement
            | Operation::PreIncrement | Operation::PreDecrement => {
                self.check_expression(node.left)?;
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

                // Engine structures can have field access too (e.g. effect properties)
                // Don't error — just return Void for unknown field access
                Ok(NwType::Void)
            }

            Operation::CondBlock => {
                // Ternary: cond ? then : else
                if node.left != NULL_NODE {
                    let cond = self.arena.get(node.left).clone();
                    self.check_expression(cond.left)?;
                }
                if node.right != NULL_NODE {
                    let choice = self.arena.get(node.right).clone();
                    let then_type = self.check_expression(choice.left)?;
                    let else_type = self.check_expression(choice.right)?;
                    if then_type != else_type {
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

            Operation::Case | Operation::Default => Ok(NwType::Void),

            _ => Ok(NwType::Void),
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
            _ => node.type_name.clone(),
        }
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
        assert_eq!(c.structs.len(), 1);
        assert_eq!(c.structs[0].name, "Vec2");
        assert_eq!(c.structs[0].fields.len(), 2);
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
