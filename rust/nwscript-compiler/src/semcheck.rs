use crate::ast::{AstArena, AstNode, NodeId, NULL_NODE, Operation};
use crate::errors::{CompileError, Diagnostic};
use crate::types::NwType;

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
        let mut lexer = crate::lexer::Lexer::new(spec, "nwscript.nss", 0);
        let tokens = lexer.tokenize();
        let mut parser = crate::parser::Parser::new(tokens);
        parser.file_names.push("nwscript.nss".to_string());
        if let Ok(root) = parser.parse_program() {
            self.collect_from_arena(root, &parser.arena);
        }
    }

    fn collect_from_arena(&mut self, node_id: NodeId, arena: &AstArena) {
        if node_id == NULL_NODE {
            return;
        }
        let node = arena.get(node_id).clone();

        match node.op {
            Operation::FunctionalUnit => {
                self.collect_from_arena(node.left, arena);
                self.collect_from_arena(node.right, arena);
            }
            Operation::FunctionDeclaration => {
                self.register_func_from_arena(node.left, false, arena);
            }
            Operation::Function => {
                self.register_func_from_arena(node.left, true, arena);
            }
            _ => {}
        }
    }

    fn register_func_from_arena(&mut self, func_id_node: NodeId, has_impl: bool, arena: &AstArena) {
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
                is_engine_action: true,
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

    fn collect_pass(&mut self, node_id: NodeId) {
        if node_id == NULL_NODE {
            return;
        }
        let node = self.arena.get(node_id).clone();

        match node.op {
            Operation::FunctionalUnit => {
                self.collect_pass(node.left);
                self.collect_pass(node.right);
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
                let size = var.nw_type.size_bytes();
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
        self.globals.push(VarEntry {
            name,
            nw_type: node.nw_type,
            type_name: node.type_name.clone(),
            scope_level: 0,
            is_constant: true,
        });
    }

    fn check_pass(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        if node_id == NULL_NODE {
            return Ok(());
        }
        let node = self.arena.get(node_id).clone();

        match node.op {
            Operation::FunctionalUnit => {
                self.check_pass(node.left)?;
                self.check_pass(node.right)?;
            }
            Operation::Function => {
                self.check_function(node_id)?;
            }
            Operation::FunctionDeclaration
            | Operation::KeywordStruct
            | Operation::GlobalVariables
            | Operation::ConstDeclaration => {
                // Already handled in collect pass
            }
            _ => {}
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

        self.var_stack.truncate(saved_var_count);
        self.scope_level -= 1;
        self.current_function = None;

        Ok(())
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
                    if ret_type != self.current_return_type
                        && self.current_return_type != NwType::Void
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
        self.check_statement(node.right)?;
        self.switch_depth -= 1;
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
                self.check_expression(node.left)?;
                self.check_expression(node.right)?;
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
                if struct_type == NwType::Struct || struct_type == NwType::Vector {
                    // Look up field type
                    if struct_type == NwType::Vector {
                        match field_name {
                            "x" | "y" | "z" => return Ok(NwType::Float),
                            _ => {
                                self.error_at(
                                    CompileError::UndefinedFieldInStructure,
                                    &node,
                                )?;
                                return Ok(NwType::Void);
                            }
                        }
                    }
                    // TODO: look up struct fields in struct definitions
                    Ok(NwType::Void)
                } else {
                    self.error_at(
                        CompileError::LeftOfStructurePartNotStructure,
                        &node,
                    )?;
                    Ok(NwType::Void)
                }
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
