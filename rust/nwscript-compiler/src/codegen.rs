use crate::ast::{AstArena, NodeId, NULL_NODE, Operation};
use crate::errors::{CompileError, Diagnostic};
use crate::opcode::Opcode;
use crate::semcheck::{FunctionSig, SemanticChecker, StructDef};
use crate::types::NwType;

const NCS_HEADER_BYTES: &[u8] = b"NCS V1.0";

#[derive(Debug, Clone)]
struct Label {
    name: String,
    offset: usize,
}

#[derive(Debug, Clone)]
struct Fixup {
    offset: usize,
    target_label: String,
}

#[derive(Debug, Clone)]
struct LocalVar {
    name: String,
    nw_type: NwType,
    type_name: Option<String>,
    stack_offset: i32,
    scope_level: u32,
    size: i32,
}

pub struct CodeGenerator<'a> {
    arena: &'a AstArena,
    file_names: &'a [String],
    code: Vec<u8>,
    pub diagnostics: Vec<Diagnostic>,
    labels: Vec<Label>,
    fixups: Vec<Fixup>,
    locals: Vec<LocalVar>,
    scope_level: u32,
    stack_depth: i32,
    base_stack_depth: i32,
    collect_all_errors: bool,
    func_sigs: Vec<FunctionSig>,
    struct_defs: Vec<StructDef>,
    current_func_name: Option<String>,
    current_return_type: NwType,
    current_return_slot_offset: i32,
    optimization_level: u32,
    reachable_functions: std::collections::HashSet<String>,
    // NDB debug data
    pub ndb_functions: Vec<crate::ndb::NdbFunctionEntry>,
    pub ndb_variables: Vec<crate::ndb::NdbVarEntry>,
    pub ndb_lines: Vec<crate::ndb::NdbLineEntry>,
    pub ndb_structs: Vec<crate::ndb::NdbStructDef>,
    current_func_start: usize,
    loop_start_stack: Vec<usize>,
    break_fixup_stack: Vec<Vec<usize>>,
    continue_fixup_stack: Vec<Vec<usize>>,
    has_globals: bool,
    global_var_size: i32,
}

impl<'a> CodeGenerator<'a> {
    pub fn new(arena: &'a AstArena, file_names: &'a [String]) -> Self {
        Self {
            arena,
            file_names,
            code: Vec::with_capacity(8192),
            diagnostics: Vec::new(),
            labels: Vec::new(),
            fixups: Vec::new(),
            locals: Vec::new(),
            scope_level: 0,
            stack_depth: 0,
            base_stack_depth: 0,
            collect_all_errors: false,
            func_sigs: Vec::new(),
            struct_defs: Vec::new(),
            current_func_name: None,
            current_return_type: NwType::Void,
            current_return_slot_offset: 0,
            optimization_level: 0,
            reachable_functions: std::collections::HashSet::new(),
            ndb_functions: Vec::new(),
            ndb_variables: Vec::new(),
            ndb_lines: Vec::new(),
            ndb_structs: Vec::new(),
            current_func_start: 0,
            loop_start_stack: Vec::new(),
            break_fixup_stack: Vec::new(),
            continue_fixup_stack: Vec::new(),
            has_globals: false,
            global_var_size: 0,
        }
    }

    pub fn set_collect_all_errors(&mut self, v: bool) {
        self.collect_all_errors = v;
    }

    pub fn set_optimization_level(&mut self, level: u32) {
        self.optimization_level = level;
    }

    pub fn load_symbols(&mut self, checker: &SemanticChecker) {
        self.func_sigs = checker.functions.clone();
        self.struct_defs = checker.structs.clone();
    }

    // ========== Low-level emission ==========

    fn emit(&mut self, b: u8) { self.code.push(b); }
    fn emit_op(&mut self, op: Opcode, aux: u8) { self.emit(op as u8); self.emit(aux); }
    fn emit_i32(&mut self, v: i32) { self.code.extend_from_slice(&v.to_be_bytes()); }
    fn emit_u16(&mut self, v: u16) { self.code.extend_from_slice(&v.to_be_bytes()); }
    fn emit_f32(&mut self, v: f32) { self.code.extend_from_slice(&v.to_be_bytes()); }
    fn emit_str(&mut self, s: &str) { self.emit_u16(s.len() as u16); self.code.extend_from_slice(s.as_bytes()); }
    fn pos(&self) -> usize { self.code.len() }

    fn patch_i32(&mut self, off: usize, v: i32) {
        let b = v.to_be_bytes();
        self.code[off] = b[0]; self.code[off+1] = b[1]; self.code[off+2] = b[2]; self.code[off+3] = b[3];
    }

    fn emit_jmp_placeholder(&mut self, op: Opcode) -> usize {
        self.emit_op(op, 0);
        let fix = self.pos();
        self.emit_i32(0);
        fix
    }

    fn patch_jmp_here(&mut self, fix: usize) {
        let target = self.pos() as i32;
        let origin = fix as i32 - 2;
        self.patch_i32(fix, target - origin);
    }

    fn emit_jmp_to(&mut self, op: Opcode, target: usize) {
        self.emit_op(op, 0);
        let origin = self.pos() as i32 - 2;
        self.emit_i32(target as i32 - origin);
    }

    fn emit_jsr_label(&mut self, label: &str) {
        self.emit_op(Opcode::Jsr, 0);
        self.fixups.push(Fixup { offset: self.pos(), target_label: label.to_string() });
        self.emit_i32(0);
    }

    fn add_label(&mut self, name: &str) {
        self.labels.push(Label { name: name.to_string(), offset: self.pos() });
    }

    fn emit_modify_sp(&mut self, amount: i32) {
        if amount != 0 {
            self.emit_op(Opcode::ModifyStackPointer, 0);
            self.emit_i32(amount);
            self.stack_depth += amount / 4;
        }
    }

    fn emit_const_int(&mut self, v: i32) {
        self.emit_op(Opcode::Constant, 0x03); self.emit_i32(v); self.stack_depth += 1;
    }
    fn emit_const_float(&mut self, v: f32) {
        self.emit_op(Opcode::Constant, 0x04); self.emit_f32(v); self.stack_depth += 1;
    }
    fn emit_const_string(&mut self, s: &str) {
        self.emit_op(Opcode::Constant, 0x05); self.emit_str(s); self.stack_depth += 1;
    }
    fn emit_const_object(&mut self, v: i32) {
        self.emit_op(Opcode::Constant, 0x06); self.emit_i32(v); self.stack_depth += 1;
    }

    fn find_local(&self, name: &str) -> Option<(i32, i32, NwType, Option<String>)> {
        self.locals.iter().rev().find(|l| l.name == name)
            .map(|l| (l.stack_offset, l.size, l.nw_type, l.type_name.clone()))
    }

    fn find_engine_func(&self, name: &str) -> Option<(u16, &FunctionSig)> {
        self.func_sigs.iter().enumerate()
            .find(|(_, f)| f.name == name && f.is_engine_action)
            .map(|(i, f)| (f.action_id as u16, f))
    }

    fn find_user_func(&self, name: &str) -> Option<&FunctionSig> {
        self.func_sigs.iter().find(|f| f.name == name && !f.is_engine_action)
    }

    fn struct_size(&self, name: &str) -> i32 {
        self.struct_defs.iter().find(|s| s.name == name).map(|s| s.byte_size).unwrap_or(0)
    }

    fn struct_field_offset(&self, struct_name: &str, field_name: &str) -> Option<(i32, i32, NwType)> {
        if let Some(sd) = self.struct_defs.iter().find(|s| s.name == struct_name) {
            for f in &sd.fields {
                if f.name == field_name {
                    return Some((f.offset, f.nw_type.size_bytes(), f.nw_type));
                }
            }
        }
        None
    }

    fn type_size(&self, nw_type: NwType, type_name: &Option<String>) -> i32 {
        match nw_type {
            NwType::Struct => type_name.as_deref().map(|n| self.struct_size(n)).unwrap_or(0),
            NwType::Vector => 12,
            _ => nw_type.size_bytes(),
        }
    }

    // ========== Main entry ==========

    pub fn generate(&mut self, root: NodeId) -> Result<Vec<u8>, CompileError> {
        self.code.clear();
        self.labels.clear();
        self.fixups.clear();

        // Header: "NCS V1.0" + 'B' + 4-byte size
        self.code.extend_from_slice(NCS_HEADER_BYTES);
        self.emit(b'B');
        let size_off = self.pos();
        self.emit_i32(0);

        // Scan for globals
        self.has_globals = self.has_global_vars(root);

        // Compute reachable functions for dead-function elimination
        if self.optimization_level & 0x01 != 0 {
            self.compute_reachable_functions(root);
        }

        // Emit loader
        self.emit_loader(root)?;

        // Emit #globals if needed
        if self.has_globals {
            self.emit_globals_func(root)?;
        }

        // Emit all user functions
        self.emit_all_functions(root)?;

        // Resolve label fixups
        self.resolve_fixups();

        // Patch file size
        self.patch_i32(size_off, self.pos() as i32);

        Ok(self.code.clone())
    }

    fn has_global_vars(&self, node_id: NodeId) -> bool {
        if node_id == NULL_NODE { return false; }
        let node = self.arena.get(node_id);
        match node.op {
            Operation::FunctionalUnit => {
                self.has_global_vars(node.left) || self.has_global_vars(node.right)
            }
            Operation::GlobalVariables => true,
            _ => false,
        }
    }

    fn entry_point_name(&self) -> &str {
        if self.func_sigs.iter().any(|f| f.name == "main") {
            "main"
        } else if self.func_sigs.iter().any(|f| f.name == "StartingConditional") {
            "StartingConditional"
        } else {
            "main"
        }
    }

    fn emit_loader(&mut self, _root: NodeId) -> Result<(), CompileError> {
        let entry = self.entry_point_name().to_string();
        if self.has_globals {
            self.emit_op(Opcode::SaveBasePointer, 0);
            self.emit_jsr_label("#globals");
            self.emit_op(Opcode::RestoreBasePointer, 0);
        } else {
            self.emit_jsr_label(&entry);
        }
        self.emit_op(Opcode::Ret, 0);
        Ok(())
    }

    fn emit_globals_func(&mut self, root: NodeId) -> Result<(), CompileError> {
        self.add_label("#globals");
        self.stack_depth = 0;
        self.global_var_size = 0;

        // Walk the tree to emit global variable initializers
        self.emit_global_var_inits(root)?;

        // After globals are initialized, call entry point
        let entry = self.entry_point_name().to_string();
        self.emit_op(Opcode::SaveBasePointer, 0);
        self.emit_jsr_label(&entry);
        self.emit_op(Opcode::RestoreBasePointer, 0);

        // Clean up globals from stack
        if self.global_var_size > 0 {
            self.emit_modify_sp(-self.global_var_size);
        }

        self.emit_op(Opcode::Ret, 0);
        Ok(())
    }

    fn emit_global_var_inits(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        if node_id == NULL_NODE { return Ok(()); }
        let node = self.arena.get(node_id).clone();
        match node.op {
            Operation::FunctionalUnit => {
                self.emit_global_var_inits(node.left)?;
                self.emit_global_var_inits(node.right)?;
            }
            Operation::GlobalVariables => {
                if node.left == NULL_NODE { return Ok(()); }
                let decl = self.arena.get(node.left).clone();
                if decl.left == NULL_NODE { return Ok(()); }
                let type_node = self.arena.get(decl.left).clone();
                let nw_type = op_to_type(type_node.op);
                let type_name = type_node.type_name.clone();
                let size = self.type_size(nw_type, &type_name);

                let mut vl_id = type_node.left;
                while vl_id != NULL_NODE {
                    let vl = self.arena.get(vl_id).clone();
                    if vl.left != NULL_NODE {
                        let var = self.arena.get(vl.left).clone();
                        let name = var.string_data.as_deref().unwrap_or("").to_string();

                        if var.left != NULL_NODE {
                            self.generate_expr(var.left)?;
                        } else {
                            self.emit_default_value(nw_type);
                        }

                        self.locals.push(LocalVar {
                            name, nw_type, type_name: type_name.clone(),
                            stack_offset: self.stack_depth * 4,
                            scope_level: 0, size,
                        });
                        self.global_var_size += size;
                    }
                    vl_id = vl.right;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn emit_default_value(&mut self, nw_type: NwType) {
        match nw_type {
            NwType::Integer => self.emit_const_int(0),
            NwType::Float => self.emit_const_float(0.0),
            NwType::String => self.emit_const_string(""),
            NwType::Object => self.emit_const_object(0x7f000000u32 as i32),
            NwType::Vector => {
                self.emit_const_float(0.0);
                self.emit_const_float(0.0);
                self.emit_const_float(0.0);
            }
            _ => self.emit_const_int(0),
        }
    }

    fn emit_all_functions(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        if node_id == NULL_NODE { return Ok(()); }
        let node = self.arena.get(node_id).clone();
        if node.op == Operation::FunctionalUnit {
            self.emit_all_functions(node.left)?;
            self.emit_all_functions(node.right)?;
            return Ok(());
        }
        if node.op == Operation::Function {
            // Dead function elimination
            if self.optimization_level & 0x01 != 0 {
                if let Some(fid_node) = self.arena.try_get(node.left) {
                    let name = fid_node.string_data.as_deref().unwrap_or("");
                    if !self.reachable_functions.contains(name) {
                        return Ok(());
                    }
                }
            }
            self.emit_function(node_id)?;
        }
        Ok(())
    }

    fn compute_reachable_functions(&mut self, root: NodeId) {
        let entry = self.entry_point_name().to_string();
        let mut worklist = vec![entry];
        if self.has_globals {
            worklist.push("#globals".to_string());
        }

        while let Some(name) = worklist.pop() {
            if !self.reachable_functions.insert(name.clone()) {
                continue;
            }
            // Find function body and collect called functions
            if let Some(body) = self.find_function_body(root, &name) {
                let mut called = Vec::new();
                self.collect_calls(body, &mut called);
                for c in called {
                    if !self.reachable_functions.contains(&c) {
                        worklist.push(c);
                    }
                }
            }
        }
    }

    fn find_function_body(&self, node_id: NodeId, name: &str) -> Option<NodeId> {
        if node_id == NULL_NODE { return None; }
        let node = self.arena.get(node_id);
        if node.op == Operation::FunctionalUnit {
            if let Some(b) = self.find_function_body(node.left, name) { return Some(b); }
            return self.find_function_body(node.right, name);
        }
        if node.op == Operation::Function && node.left != NULL_NODE {
            let fid = self.arena.get(node.left);
            if fid.string_data.as_deref() == Some(name) {
                return Some(node.right);
            }
        }
        None
    }

    fn collect_calls(&self, node_id: NodeId, out: &mut Vec<String>) {
        if node_id == NULL_NODE { return; }
        let node = self.arena.get(node_id);
        if node.op == Operation::Action && node.left != NULL_NODE {
            let aid = self.arena.get(node.left);
            if let Some(n) = &aid.string_data {
                // Only collect user functions, not engine actions
                if self.func_sigs.iter().any(|f| f.name == *n && !f.is_engine_action) {
                    out.push(n.clone());
                }
            }
        }
        self.collect_calls(node.left, out);
        self.collect_calls(node.right, out);
    }

    fn emit_function(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        if node.left == NULL_NODE { return Ok(()); }
        let fid = self.arena.get(node.left).clone();
        let func_name = fid.string_data.as_deref().unwrap_or("").to_string();
        let return_type = fid.nw_type;

        self.add_label(&func_name);
        self.current_func_name = Some(func_name.clone());
        self.current_return_type = return_type;
        self.current_func_start = self.pos();

        let saved_locals = self.locals.len();
        let saved_depth = self.stack_depth;
        self.stack_depth = 0;
        self.base_stack_depth = 0;
        self.scope_level = 1;

        // Register parameters as locals (caller pushed them before JSR)
        let mut params = Vec::new();
        let mut param_node = fid.left;
        while param_node != NULL_NODE {
            let p = self.arena.get(param_node).clone();
            if p.op == Operation::FunctionParamName {
                let name = p.string_data.as_deref().unwrap_or("").to_string();
                let size = self.type_size(p.nw_type, &p.type_name);
                params.push((name, p.nw_type, p.type_name.clone(), size));
            }
            param_node = p.right;
        }

        // Parameters are on stack below the return address, addressed with negative offsets
        let total_param_size: i32 = params.iter().map(|p| p.3).sum();
        let mut param_offset = -total_param_size;
        for (name, nw_type, type_name, size) in &params {
            self.locals.push(LocalVar {
                name: name.clone(), nw_type: *nw_type, type_name: type_name.clone(),
                stack_offset: param_offset, scope_level: 1, size: *size,
            });
            param_offset += size;
        }

        // Track where the caller's reserved return slot is, relative to function entry.
        // Caller pushed RUNSTACK_ADD before args, so the return slot is below all params.
        let return_size = self.type_size(return_type, &fid.type_name.clone());
        self.current_return_slot_offset = -total_param_size - return_size;

        // Generate function body
        if node.right != NULL_NODE {
            self.generate_stmt(node.right)?;
        }

        // Epilogue: clean up any leftover locals and return
        let local_alloc: i32 = self.locals[saved_locals..].iter()
            .filter(|l| l.scope_level > 0 && l.stack_offset >= 0)
            .map(|l| l.size).sum();
        if local_alloc > 0 {
            self.emit_modify_sp(-local_alloc);
        }

        self.emit_op(Opcode::Ret, 0);

        // Record NDB function entry
        let func_end = self.pos();
        let ndb_params: Vec<_> = params.iter()
            .map(|(_, t, n, _)| (*t, n.clone().unwrap_or_default()))
            .collect();
        self.ndb_functions.push(crate::ndb::NdbFunctionEntry {
            name: func_name.clone(),
            return_type,
            return_struct_name: fid.type_name.clone().unwrap_or_default(),
            code_start: self.current_func_start as u32,
            code_end: func_end as u32,
            params: ndb_params,
        });

        self.locals.truncate(saved_locals);
        self.stack_depth = saved_depth;
        self.scope_level = 0;
        self.current_func_name = None;
        Ok(())
    }

    fn resolve_fixups(&mut self) {
        let patches: Vec<(usize, i32)> = self.fixups.iter().filter_map(|fixup| {
            self.labels.iter().find(|l| l.name == fixup.target_label).map(|label| {
                let target = label.offset as i32;
                let origin = fixup.offset as i32 - 2;
                (fixup.offset, target - origin)
            })
        }).collect();
        for (off, val) in patches {
            self.patch_i32(off, val);
        }
    }

    // ========== Statement codegen ==========

    fn generate_stmt(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        if node_id == NULL_NODE { return Ok(()); }
        let node = self.arena.get(node_id).clone();

        match node.op {
            Operation::CompoundStatement => {
                self.scope_level += 1;
                let saved = self.locals.len();
                let saved_ndb_vars = self.ndb_variables.len();
                let saved_depth = self.stack_depth;
                self.generate_stmt(node.left)?;
                let alloc: i32 = self.locals[saved..].iter().map(|l| l.size).sum();
                if alloc > 0 { self.emit_modify_sp(-alloc); }
                // Patch NDB var end positions for vars that went out of scope here
                let end_pos = self.pos() as u32;
                for v in &mut self.ndb_variables[saved_ndb_vars..] {
                    if v.code_end == 0 {
                        v.code_end = end_pos;
                    }
                }
                self.locals.truncate(saved);
                self.scope_level -= 1;
                self.stack_depth = saved_depth;
            }
            Operation::StatementList => {
                self.generate_stmt(node.left)?;
                self.generate_stmt(node.right)?;
            }
            Operation::Statement | Operation::StatementNoDebug => {
                let line_start = self.pos();
                let line = node.line;
                let file_id = node.file_id;
                self.generate_stmt(node.left)?;
                let line_end = self.pos();
                if node.op == Operation::Statement && line > 0 && line_end > line_start {
                    self.ndb_lines.push(crate::ndb::NdbLineEntry {
                        file_id: file_id as u8,
                        line,
                        code_start: line_start as u32,
                        code_end: line_end as u32,
                    });
                }
            }
            Operation::KeywordDeclaration | Operation::ConstDeclaration => {
                self.gen_local_decl(node_id)?;
            }
            Operation::IfBlock => self.gen_if(node_id)?,
            Operation::WhileBlock => self.gen_while(node_id)?,
            Operation::DoWhileBlock => self.gen_do_while(node_id)?,
            Operation::ForBlock => self.generate_stmt(node.left)?,
            Operation::SwitchBlock => self.gen_switch(node_id)?,
            Operation::Return => self.gen_return(node_id)?,
            Operation::Break => {
                let fix = self.emit_jmp_placeholder(Opcode::Jmp);
                if let Some(exits) = self.break_fixup_stack.last_mut() {
                    exits.push(fix);
                }
            }
            Operation::Continue => {
                if let Some(&target) = self.loop_start_stack.last() {
                    if target == usize::MAX {
                        // For-loop: continue jumps to update expression — register a fixup
                        let fix = self.emit_jmp_placeholder(Opcode::Jmp);
                        if let Some(fixups) = self.continue_fixup_stack.last_mut() {
                            fixups.push(fix);
                        }
                    } else {
                        self.emit_jmp_to(Opcode::Jmp, target);
                    }
                }
            }
            _ => { self.generate_expr(node_id)?; }
        }
        Ok(())
    }

    fn gen_local_decl(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        if node.left == NULL_NODE { return Ok(()); }
        let type_node = self.arena.get(node.left).clone();
        let nw_type = op_to_type(type_node.op);
        let type_name = type_node.type_name.clone();
        let size = self.type_size(nw_type, &type_name);

        let mut vl_id = type_node.left;
        while vl_id != NULL_NODE {
            let vl = self.arena.get(vl_id).clone();
            if vl.left != NULL_NODE {
                let var = self.arena.get(vl.left).clone();
                let name = var.string_data.as_deref().unwrap_or("").to_string();
                if var.left != NULL_NODE {
                    self.generate_expr(var.left)?;
                } else {
                    self.emit_default_value(nw_type);
                }
                let offset = self.stack_depth * 4;

                // NDB: record variable lifetime starting here
                let code_start = self.pos();
                self.ndb_variables.push(crate::ndb::NdbVarEntry {
                    name: name.clone(),
                    var_type: nw_type,
                    struct_name: type_name.clone().unwrap_or_default(),
                    stack_loc: offset as u32,
                    code_start: code_start as u32,
                    code_end: 0, // patched when scope exits
                });

                self.locals.push(LocalVar {
                    name, nw_type, type_name: type_name.clone(),
                    stack_offset: offset, scope_level: self.scope_level, size,
                });
            }
            vl_id = vl.right;
        }
        Ok(())
    }

    fn gen_if(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        if node.left != NULL_NODE {
            let cond = self.arena.get(node.left).clone();
            if cond.left != NULL_NODE { self.generate_expr(cond.left)?; }
        }
        let jz = self.emit_jmp_placeholder(Opcode::Jz);
        self.stack_depth -= 1;
        if node.right != NULL_NODE {
            let choice = self.arena.get(node.right).clone();
            self.generate_stmt(choice.left)?;
            if choice.right != NULL_NODE {
                let jmp = self.emit_jmp_placeholder(Opcode::Jmp);
                self.patch_jmp_here(jz);
                self.generate_stmt(choice.right)?;
                self.patch_jmp_here(jmp);
            } else {
                self.patch_jmp_here(jz);
            }
        } else {
            self.patch_jmp_here(jz);
        }
        Ok(())
    }

    fn gen_while(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        let loop_top = self.pos();

        // For-loop detection: extract the WhileContinue (update) node if present
        // so we can use it as the continue target instead of loop_top
        let (body_root, update_node) = self.extract_for_update(node.right);

        // Push continue target placeholder — we'll update it after we know where update is.
        // For now use loop_top; if there's no update, that's correct.
        let continue_target_placeholder = if update_node != NULL_NODE {
            // We'll patch after we emit the update
            usize::MAX
        } else {
            loop_top
        };
        self.loop_start_stack.push(continue_target_placeholder);
        self.break_fixup_stack.push(Vec::new());
        // Track break fixups also for continue if it's a for-loop
        // (continue jumps to update which is after body but before back-jump)
        let continue_fixups_idx = if update_node != NULL_NODE {
            self.continue_fixup_stack.push(Vec::new());
            Some(self.continue_fixup_stack.len() - 1)
        } else {
            None
        };

        if node.left != NULL_NODE {
            let cond = self.arena.get(node.left).clone();
            if cond.left != NULL_NODE { self.generate_expr(cond.left)?; }
        }
        let jz = self.emit_jmp_placeholder(Opcode::Jz);
        self.stack_depth -= 1;

        // Body
        if body_root != NULL_NODE {
            self.generate_stmt(body_root)?;
        }

        // Update (continue target for for-loops)
        if update_node != NULL_NODE {
            let update_pos = self.pos();
            // Patch continue fixups to jump here
            if let Some(idx) = continue_fixups_idx {
                let fixups = std::mem::take(&mut self.continue_fixup_stack[idx]);
                for f in fixups {
                    self.patch_jmp_here(f);
                }
                // Also update loop_start_stack so any nested continue uses update_pos
                if let Some(top) = self.loop_start_stack.last_mut() {
                    *top = update_pos;
                }
            }
            // Emit the update expression
            let upd = self.arena.get(update_node).clone();
            if upd.left != NULL_NODE {
                self.generate_expr(upd.left)?;
            }
        }

        self.emit_jmp_to(Opcode::Jmp, loop_top);
        self.patch_jmp_here(jz);

        for f in self.break_fixup_stack.pop().unwrap_or_default() { self.patch_jmp_here(f); }
        if continue_fixups_idx.is_some() {
            self.continue_fixup_stack.pop();
        }
        self.loop_start_stack.pop();
        Ok(())
    }

    /// If the while body is a desugared for-loop, split out the WhileContinue.
    /// Returns (body_without_update, update_node).
    fn extract_for_update(&self, choice_node: NodeId) -> (NodeId, NodeId) {
        if choice_node == NULL_NODE { return (NULL_NODE, NULL_NODE); }
        let choice = self.arena.get(choice_node);
        // choice.left is CompoundStatement
        if choice.left == NULL_NODE { return (choice_node, NULL_NODE); }
        let cs = self.arena.get(choice.left);
        if cs.op != Operation::CompoundStatement { return (choice_node, NULL_NODE); }
        // cs.left is StatementList: [body, [WhileContinue]]
        let sl = cs.left;
        if sl == NULL_NODE { return (choice_node, NULL_NODE); }
        let sl_node = self.arena.get(sl);
        if sl_node.op != Operation::StatementList { return (choice_node, NULL_NODE); }
        // Right side should be another StatementList wrapping WhileContinue
        let right_sl = sl_node.right;
        if right_sl == NULL_NODE { return (choice_node, NULL_NODE); }
        let right_node = self.arena.get(right_sl);
        if right_node.op != Operation::StatementList { return (choice_node, NULL_NODE); }
        let candidate = right_node.left;
        if candidate == NULL_NODE { return (choice_node, NULL_NODE); }
        let cand_node = self.arena.get(candidate);
        if cand_node.op == Operation::WhileContinue {
            // body is sl_node.left only
            (sl_node.left, candidate)
        } else {
            (choice_node, NULL_NODE)
        }
    }

    fn gen_do_while(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        let loop_top = self.pos();
        self.loop_start_stack.push(loop_top);
        self.break_fixup_stack.push(Vec::new());

        self.generate_stmt(node.left)?;

        if node.right != NULL_NODE {
            let cond = self.arena.get(node.right).clone();
            if cond.left != NULL_NODE { self.generate_expr(cond.left)?; }
        }
        self.emit_jmp_to(Opcode::Jnz, loop_top);
        self.stack_depth -= 1;

        for f in self.break_fixup_stack.pop().unwrap_or_default() { self.patch_jmp_here(f); }
        self.loop_start_stack.pop();
        Ok(())
    }

    fn gen_switch(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        self.break_fixup_stack.push(Vec::new());

        if node.left != NULL_NODE {
            let cond = self.arena.get(node.left).clone();
            if cond.left != NULL_NODE { self.generate_expr(cond.left)?; }
        }
        self.gen_switch_body(node.right)?;
        self.emit_modify_sp(-4); // pop switch expression

        for f in self.break_fixup_stack.pop().unwrap_or_default() { self.patch_jmp_here(f); }
        Ok(())
    }

    fn gen_switch_body(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        // Collect all items in the switch body into a flat list
        let mut items = Vec::new();
        self.flatten_switch_items(node_id, &mut items);

        let mut case_miss_fixup: Option<usize> = None;

        for item_id in items {
            let item = self.arena.get(item_id).clone();
            match item.op {
                Operation::Case => {
                    // Patch previous case-miss to jump here
                    if let Some(fixup) = case_miss_fixup.take() {
                        self.patch_jmp_here(fixup);
                    }
                    // Dup switch expr, compare with case value
                    self.emit_op(Opcode::RunstackCopy, 0);
                    self.emit_i32(-4); self.emit_u16(4);
                    self.stack_depth += 1;
                    if item.left != NULL_NODE { self.generate_expr(item.left)?; }
                    self.emit_op(Opcode::Equal, 0x20);
                    self.stack_depth -= 1;
                    let jz = self.emit_jmp_placeholder(Opcode::Jz);
                    self.stack_depth -= 1;
                    case_miss_fixup = Some(jz);
                }
                Operation::Default => {
                    if let Some(fixup) = case_miss_fixup.take() {
                        self.patch_jmp_here(fixup);
                    }
                }
                _ => {
                    self.generate_stmt(item_id)?;
                }
            }
        }

        // Patch final case-miss to fall through to switch exit
        if let Some(fixup) = case_miss_fixup {
            self.patch_jmp_here(fixup);
        }

        Ok(())
    }

    fn flatten_switch_items(&self, node_id: NodeId, items: &mut Vec<NodeId>) {
        if node_id == NULL_NODE { return; }
        let node = self.arena.get(node_id);
        if node.op == Operation::StatementList {
            self.flatten_switch_items(node.left, items);
            self.flatten_switch_items(node.right, items);
        } else {
            items.push(node_id);
        }
    }

    fn gen_return(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        if node.left != NULL_NODE {
            let ret_size = self.type_size(self.current_return_type, &None);
            self.generate_expr(node.left)?;

            // Copy return value to caller's reserved slot.
            // The slot offset was computed relative to function entry SP=0;
            // adjust for current SP which includes the just-pushed return value.
            if ret_size > 0 {
                let target_offset = self.current_return_slot_offset - self.stack_depth * 4;
                self.emit_op(Opcode::Assignment, 0);
                self.emit_i32(target_offset);
                self.emit_u16(ret_size as u16);
                // The just-pushed value gets consumed by the ASSIGNMENT? In NWScript VM,
                // ASSIGNMENT copies but doesn't pop. Pop it manually.
                self.emit_modify_sp(-ret_size);
            }
        }
        // Clean up local variables
        let local_size: i32 = self.locals.iter()
            .filter(|l| l.scope_level > 0 && l.stack_offset >= 0)
            .map(|l| l.size).sum();
        if local_size > 0 {
            self.emit_modify_sp(-local_size);
        }
        self.emit_op(Opcode::Ret, 0);
        Ok(())
    }

    // ========== Expression codegen ==========

    fn generate_expr(&mut self, node_id: NodeId) -> Result<NwType, CompileError> {
        if node_id == NULL_NODE { return Ok(NwType::Void); }
        let node = self.arena.get(node_id).clone();

        match node.op {
            // ---- Constants ----
            Operation::ConstantInteger => {
                self.emit_const_int(node.int_data[0]); Ok(NwType::Integer)
            }
            Operation::ConstantFloat => {
                self.emit_const_float(node.float_data); Ok(NwType::Float)
            }
            Operation::ConstantString => {
                self.emit_const_string(node.string_data.as_deref().unwrap_or("")); Ok(NwType::String)
            }
            Operation::ConstantObject => {
                self.emit_const_object(node.int_data[0]); Ok(NwType::Object)
            }
            Operation::ConstantVector => {
                // Evaluate the three sub-expressions
                if node.left != NULL_NODE {
                    self.gen_vector_args(node.left)?;
                } else {
                    self.emit_const_float(node.vector_data[0]);
                    self.emit_const_float(node.vector_data[1]);
                    self.emit_const_float(node.vector_data[2]);
                }
                Ok(NwType::Vector)
            }
            Operation::ConstantJson => {
                self.emit_op(Opcode::Constant, 0x17); // ENGST7 = json
                let payload = node.string_data.as_deref().unwrap_or("");
                self.emit_str(payload);
                self.stack_depth += 1;
                Ok(NwType::EngineStructure(7))
            }
            Operation::ConstantLocation => {
                self.emit_op(Opcode::Constant, 0x12);
                self.emit_i32(0);
                self.stack_depth += 1;
                Ok(NwType::EngineStructure(2))
            }

            // ---- Variables ----
            Operation::Variable => {
                let name = node.string_data.as_deref().unwrap_or("");
                if let Some((so, sz, nt, _tn)) = self.find_local(name) {
                    let offset = so - self.stack_depth * 4;
                    self.emit_op(Opcode::RunstackCopy, 0);
                    self.emit_i32(offset);
                    self.emit_u16(sz as u16);
                    self.stack_depth += sz / 4;
                    Ok(nt)
                } else {
                    // Global variable — use base-pointer-relative access
                    // Globals are below the base pointer, addressed with RunstackCopyBase
                    self.emit_op(Opcode::RunstackCopyBase, 0);
                    self.emit_i32(-(self.global_var_size)); // offset from base pointer
                    self.emit_u16(4);
                    self.stack_depth += 1;
                    Ok(NwType::Integer)
                }
            }

            // ---- Assignment ----
            Operation::Assignment => {
                let op_token = node.int_data[0];
                let is_compound = op_token != crate::token::TokenType::AssignmentEqual as i32
                    && op_token != 0;

                if is_compound && node.left != NULL_NODE {
                    // Compound assignment (+=, -=, etc.): load current value first
                    self.generate_expr(node.left)?;
                }

                let rt = self.generate_expr(node.right)?;

                if is_compound {
                    // Apply the operator
                    let compound_op = match op_token {
                        x if x == crate::token::TokenType::AssignmentPlus as i32 => Some(Opcode::Add),
                        x if x == crate::token::TokenType::AssignmentMinus as i32 => Some(Opcode::Sub),
                        x if x == crate::token::TokenType::AssignmentMultiply as i32 => Some(Opcode::Mul),
                        x if x == crate::token::TokenType::AssignmentDivide as i32 => Some(Opcode::Div),
                        x if x == crate::token::TokenType::AssignmentModulus as i32 => Some(Opcode::Modulus),
                        x if x == crate::token::TokenType::AssignmentAnd as i32 => Some(Opcode::BooleanAnd),
                        x if x == crate::token::TokenType::AssignmentOr as i32 => Some(Opcode::InclusiveOr),
                        x if x == crate::token::TokenType::AssignmentXor as i32 => Some(Opcode::ExclusiveOr),
                        x if x == crate::token::TokenType::AssignmentShiftLeft as i32 => Some(Opcode::ShiftLeft),
                        x if x == crate::token::TokenType::AssignmentShiftRight as i32 => Some(Opcode::ShiftRight),
                        x if x == crate::token::TokenType::AssignmentUShiftRight as i32 => Some(Opcode::UShiftRight),
                        _ => None,
                    };
                    if let Some(op) = compound_op {
                        self.emit_op(op, 0x20);
                        self.stack_depth -= 1;
                    }
                }

                if node.left != NULL_NODE {
                    let lhs = self.arena.get(node.left).clone();
                    if lhs.op == Operation::Variable {
                        let name = lhs.string_data.as_deref().unwrap_or("");
                        if let Some((so, sz, _, _)) = self.find_local(name) {
                            let offset = so - self.stack_depth * 4;
                            self.emit_op(Opcode::Assignment, 0);
                            self.emit_i32(offset);
                            self.emit_u16(sz as u16);
                        }
                    } else if lhs.op == Operation::StructurePart {
                        self.gen_struct_field_assign(&lhs)?;
                    }
                }
                Ok(rt)
            }

            // ---- Arithmetic ----
            Operation::Add | Operation::Subtract | Operation::Multiply
            | Operation::Divide | Operation::Modulus => {
                let lt = self.generate_expr(node.left)?;
                let rt = self.generate_expr(node.right)?;
                let op = match node.op {
                    Operation::Add => Opcode::Add,
                    Operation::Subtract => Opcode::Sub,
                    Operation::Multiply => Opcode::Mul,
                    Operation::Divide => Opcode::Div,
                    Operation::Modulus => Opcode::Modulus,
                    _ => unreachable!(),
                };
                self.emit_op(op, lt.auxcode_pair(rt).unwrap_or(0x20));
                self.stack_depth -= 1;
                Ok(if lt == NwType::Float || rt == NwType::Float { NwType::Float } else { lt })
            }
            Operation::Negation => {
                let t = self.generate_expr(node.left)?;
                self.emit_op(Opcode::Negation, t.auxcode());
                Ok(t)
            }
            Operation::BooleanNot => {
                self.generate_expr(node.left)?;
                self.emit_op(Opcode::BooleanNot, 0x03);
                Ok(NwType::Integer)
            }
            Operation::OnesComplement => {
                self.generate_expr(node.left)?;
                self.emit_op(Opcode::OnesComplement, 0x03);
                Ok(NwType::Integer)
            }

            // ---- Logical (short-circuit) ----
            Operation::LogicalAnd => {
                self.generate_expr(node.left)?;
                let jz = self.emit_jmp_placeholder(Opcode::Jz);
                self.stack_depth -= 1;
                self.generate_expr(node.right)?;
                let jmp = self.emit_jmp_placeholder(Opcode::Jmp);
                self.stack_depth -= 1;
                self.patch_jmp_here(jz);
                self.emit_const_int(0);
                self.patch_jmp_here(jmp);
                Ok(NwType::Integer)
            }
            Operation::LogicalOr => {
                self.generate_expr(node.left)?;
                let jnz = self.emit_jmp_placeholder(Opcode::Jnz);
                self.stack_depth -= 1;
                self.generate_expr(node.right)?;
                let jmp = self.emit_jmp_placeholder(Opcode::Jmp);
                self.stack_depth -= 1;
                self.patch_jmp_here(jnz);
                self.emit_const_int(1);
                self.patch_jmp_here(jmp);
                Ok(NwType::Integer)
            }

            // ---- Bitwise ----
            Operation::InclusiveOr | Operation::ExclusiveOr | Operation::BooleanAnd
            | Operation::ShiftLeft | Operation::ShiftRight | Operation::UnsignedShiftRight => {
                self.generate_expr(node.left)?;
                self.generate_expr(node.right)?;
                let op = match node.op {
                    Operation::InclusiveOr => Opcode::InclusiveOr,
                    Operation::ExclusiveOr => Opcode::ExclusiveOr,
                    Operation::BooleanAnd => Opcode::BooleanAnd,
                    Operation::ShiftLeft => Opcode::ShiftLeft,
                    Operation::ShiftRight => Opcode::ShiftRight,
                    Operation::UnsignedShiftRight => Opcode::UShiftRight,
                    _ => unreachable!(),
                };
                self.emit_op(op, 0x20);
                self.stack_depth -= 1;
                Ok(NwType::Integer)
            }

            // ---- Comparison ----
            Operation::ConditionEqual | Operation::ConditionNotEqual
            | Operation::ConditionGEQ | Operation::ConditionGT
            | Operation::ConditionLT | Operation::ConditionLEQ => {
                let lt = self.generate_expr(node.left)?;
                let rt = self.generate_expr(node.right)?;
                let op = match node.op {
                    Operation::ConditionEqual => Opcode::Equal,
                    Operation::ConditionNotEqual => Opcode::NotEqual,
                    Operation::ConditionGEQ => Opcode::GEQ,
                    Operation::ConditionGT => Opcode::GT,
                    Operation::ConditionLT => Opcode::LT,
                    Operation::ConditionLEQ => Opcode::LEQ,
                    _ => unreachable!(),
                };
                self.emit_op(op, lt.auxcode_pair(rt).unwrap_or(0x20));
                self.stack_depth -= 1;
                Ok(NwType::Integer)
            }

            // ---- Increment/Decrement ----
            Operation::PreIncrement => self.gen_inc_dec(node_id, Opcode::Increment, true),
            Operation::PostIncrement => self.gen_inc_dec(node_id, Opcode::Increment, false),
            Operation::PreDecrement => self.gen_inc_dec(node_id, Opcode::Decrement, true),
            Operation::PostDecrement => self.gen_inc_dec(node_id, Opcode::Decrement, false),

            // ---- Function call ----
            Operation::Action => self.gen_call(node_id),

            // ---- Ternary ----
            Operation::CondBlock => {
                if node.left != NULL_NODE {
                    let cond = self.arena.get(node.left).clone();
                    self.generate_expr(cond.left)?;
                }
                let jz = self.emit_jmp_placeholder(Opcode::Jz);
                self.stack_depth -= 1;
                let mut rt = NwType::Void;
                if node.right != NULL_NODE {
                    let choice = self.arena.get(node.right).clone();
                    rt = self.generate_expr(choice.left)?;
                    let jmp = self.emit_jmp_placeholder(Opcode::Jmp);
                    self.stack_depth -= 1;
                    self.patch_jmp_here(jz);
                    self.generate_expr(choice.right)?;
                    self.patch_jmp_here(jmp);
                } else {
                    self.patch_jmp_here(jz);
                }
                Ok(rt)
            }

            // ---- Struct field access ----
            Operation::StructurePart => self.gen_struct_field_read(node_id),

            Operation::ActionArgList => self.generate_expr(node.left),
            _ => Ok(NwType::Void),
        }
    }

    fn gen_vector_args(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        if node_id == NULL_NODE { return Ok(()); }
        let node = self.arena.get(node_id).clone();
        if node.op == Operation::ActionArgList {
            if node.left != NULL_NODE { self.generate_expr(node.left)?; }
            if node.right != NULL_NODE { self.gen_vector_args(node.right)?; }
        }
        Ok(())
    }

    fn gen_inc_dec(&mut self, node_id: NodeId, opcode: Opcode, is_pre: bool) -> Result<NwType, CompileError> {
        let node = self.arena.get(node_id).clone();
        if node.left != NULL_NODE {
            let lhs = self.arena.get(node.left).clone();
            if lhs.op == Operation::Variable {
                let name = lhs.string_data.as_deref().unwrap_or("");
                if let Some((so, sz, _, _)) = self.find_local(name) {
                    if !is_pre {
                        // post: push old value first
                        let offset = so - self.stack_depth * 4;
                        self.emit_op(Opcode::RunstackCopy, 0);
                        self.emit_i32(offset);
                        self.emit_u16(sz as u16);
                        self.stack_depth += sz / 4;
                    }
                    let offset = so - self.stack_depth * 4;
                    self.emit_op(opcode, 0x03);
                    self.emit_i32(offset);
                    if is_pre {
                        // pre: push new value after increment
                        let offset = so - self.stack_depth * 4;
                        self.emit_op(Opcode::RunstackCopy, 0);
                        self.emit_i32(offset);
                        self.emit_u16(sz as u16);
                        self.stack_depth += sz / 4;
                    }
                }
            }
        }
        Ok(NwType::Integer)
    }

    fn gen_call(&mut self, node_id: NodeId) -> Result<NwType, CompileError> {
        let node = self.arena.get(node_id).clone();
        if node.left == NULL_NODE { return Ok(NwType::Void); }
        let aid = self.arena.get(node.left).clone();
        let func_name = aid.string_data.as_deref().unwrap_or("").to_string();

        // Generate arguments, tracking each one's type for proper stack cleanup
        let mut arg_count = 0u8;
        let mut arg_types: Vec<NwType> = Vec::new();
        let mut arg_node = aid.right;
        while arg_node != NULL_NODE {
            let arg = self.arena.get(arg_node).clone();
            if arg.left != NULL_NODE {
                let t = self.generate_expr(arg.left)?;
                arg_types.push(t);
                arg_count += 1;
            }
            arg_node = arg.right;
        }

        let arg_byte_size: i32 = arg_types.iter()
            .map(|t| self.type_size(*t, &None))
            .sum();
        let arg_slot_count: i32 = arg_byte_size / 4;

        // Check if it's an engine function
        if let Some((action_id, sig)) = self.find_engine_func(&func_name).map(|(id, s)| (id, s.clone())) {
            self.emit_op(Opcode::ExecuteCommand, 0);
            self.emit_u16(action_id);
            self.emit(arg_count);
            self.stack_depth -= arg_slot_count;
            if sig.return_type != NwType::Void {
                let ret_slots = self.type_size(sig.return_type, &sig.return_type_name) / 4;
                self.stack_depth += ret_slots;
            }
            return Ok(sig.return_type);
        }

        // User-defined function
        if let Some(sig) = self.find_user_func(&func_name).cloned() {
            // Reserve return value space if non-void
            if sig.return_type != NwType::Void {
                let ret_slots = self.type_size(sig.return_type, &sig.return_type_name) / 4;
                for _ in 0..ret_slots {
                    self.emit_op(Opcode::RunstackAdd, sig.return_type.auxcode());
                    self.stack_depth += 1;
                }
            }

            self.emit_jsr_label(&func_name);

            // Clean up arguments using actual sizes
            if arg_byte_size > 0 {
                self.emit_modify_sp(-arg_byte_size);
            }
            return Ok(sig.return_type);
        }

        // Unknown function — emit placeholder
        self.emit_op(Opcode::ExecuteCommand, 0);
        self.emit_u16(0);
        self.emit(arg_count);
        self.stack_depth -= arg_slot_count;
        Ok(NwType::Void)
    }

    fn gen_struct_field_read(&mut self, node_id: NodeId) -> Result<NwType, CompileError> {
        let node = self.arena.get(node_id).clone();
        let field_name = node.string_data.as_deref().unwrap_or("");

        // Generate the struct value onto the stack
        let struct_type = self.generate_expr(node.left)?;

        if struct_type == NwType::Vector {
            // Vector field access: x=0, y=4, z=8
            let field_offset = match field_name {
                "x" => 0i32,
                "y" => 4,
                "z" => 8,
                _ => return Ok(NwType::Void),
            };
            // DE_STRUCT: extract one float from the 12-byte vector
            self.emit_op(Opcode::DeStruct, 0);
            self.emit_i32(4);  // size to keep
            self.emit_i32(field_offset); // offset of field
            self.emit_i32(12); // total struct size
            self.stack_depth -= 2; // remove 3 slots, add 1
            return Ok(NwType::Float);
        }

        // Struct field access
        if struct_type == NwType::Struct {
            if let Some(lhs_node) = self.arena.try_get(node.left) {
                let type_name = lhs_node.type_name.as_deref().unwrap_or("");
                if let Some((field_off, field_sz, field_type)) = self.struct_field_offset(type_name, field_name) {
                    let struct_sz = self.struct_size(type_name);
                    self.emit_op(Opcode::DeStruct, 0);
                    self.emit_i32(field_sz);
                    self.emit_i32(field_off);
                    self.emit_i32(struct_sz);
                    self.stack_depth -= (struct_sz / 4) - (field_sz / 4);
                    return Ok(field_type);
                }
            }
        }

        Ok(NwType::Void)
    }

    fn gen_struct_field_assign(&mut self, lhs: &crate::ast::AstNode) -> Result<(), CompileError> {
        // lhs is a StructurePart node: lhs.left = struct expression, lhs.string_data = field name
        // The new value is already pushed on the stack by the caller.
        let field_name = lhs.string_data.as_deref().unwrap_or("");
        if lhs.left == NULL_NODE { return Ok(()); }

        let target = self.arena.get(lhs.left).clone();
        if target.op != Operation::Variable {
            return Ok(()); // chained field assignment not yet supported
        }

        let var_name = target.string_data.as_deref().unwrap_or("");
        let (so, _struct_sz, _, type_name) = match self.find_local(var_name) {
            Some(v) => v,
            None => return Ok(()),
        };

        // Determine field offset and size
        let (field_off, field_sz) = if let Some(tn) = &type_name {
            if let Some((off, sz, _)) = self.struct_field_offset(tn, field_name) {
                (off, sz)
            } else {
                return Ok(());
            }
        } else {
            // Vector: x=0/4, y=4/4, z=8/4
            match field_name {
                "x" => (0i32, 4i32),
                "y" => (4, 4),
                "z" => (8, 4),
                _ => return Ok(()),
            }
        };

        // ASSIGNMENT instruction:
        // - offset is relative to current SP, pointing to the field location
        //   struct base is at `so`, field is at `so + field_off`
        // - the value to assign is on top of stack
        let target_offset = so + field_off - self.stack_depth * 4;
        self.emit_op(Opcode::Assignment, 0);
        self.emit_i32(target_offset);
        self.emit_u16(field_sz as u16);
        Ok(())
    }
}

fn op_to_type(op: Operation) -> NwType {
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
    use crate::semcheck::SemanticChecker;

    fn compile_to_ncs(src: &str) -> Vec<u8> {
        let mut lexer = Lexer::new(src, "test.nss", 0);
        let tokens = lexer.tokenize();
        let mut parser = Parser::new(tokens);
        parser.file_names.push("test.nss".to_string());
        let root = parser.parse_program().unwrap();

        let mut checker = SemanticChecker::new(&parser.arena, &parser.file_names);
        checker.set_require_entry_point(false);
        let _ = checker.check(root);

        let mut cg = CodeGenerator::new(&parser.arena, &parser.file_names);
        cg.load_symbols(&checker);
        cg.generate(root).unwrap()
    }

    #[test]
    fn test_ncs_header() {
        let ncs = compile_to_ncs("void main() { }");
        assert!(ncs.len() >= 13);
        assert_eq!(&ncs[0..8], b"NCS V1.0");
        assert_eq!(ncs[8], b'B');
    }

    #[test]
    fn test_ncs_size_in_header() {
        let ncs = compile_to_ncs("void main() { }");
        let size = i32::from_be_bytes([ncs[9], ncs[10], ncs[11], ncs[12]]);
        assert_eq!(size as usize, ncs.len());
    }

    #[test]
    fn test_deterministic() {
        let src = "void main() { int x = 1 + 2; }";
        assert_eq!(compile_to_ncs(src), compile_to_ncs(src));
    }

    #[test]
    fn test_const_int() {
        let ncs = compile_to_ncs("void main() { int x = 42; }");
        assert!(ncs.windows(2).any(|w| w[0] == Opcode::Constant as u8 && w[1] == 0x03));
    }

    #[test]
    fn test_const_string() {
        let ncs = compile_to_ncs(r#"void main() { string s = "hello"; }"#);
        assert!(ncs.windows(5).any(|w| w == b"hello"));
    }

    #[test]
    fn test_add() {
        let ncs = compile_to_ncs("void main() { int x = 1 + 2; }");
        assert!(ncs.contains(&(Opcode::Add as u8)));
    }

    #[test]
    fn test_if_jz() {
        let ncs = compile_to_ncs("void main() { if (1) { int x = 1; } }");
        assert!(ncs.contains(&(Opcode::Jz as u8)));
    }

    #[test]
    fn test_while_jmp() {
        let ncs = compile_to_ncs("void main() { while (0) { } }");
        assert!(ncs.contains(&(Opcode::Jz as u8)));
        assert!(ncs.contains(&(Opcode::Jmp as u8)));
    }

    #[test]
    fn test_ret() {
        let ncs = compile_to_ncs("void main() { }");
        assert!(ncs.contains(&(Opcode::Ret as u8)));
    }

    #[test]
    fn test_multiple_functions_jsr() {
        let ncs = compile_to_ncs("int helper() { return 42; }\nvoid main() { int x = helper(); }");
        assert!(ncs.contains(&(Opcode::Jsr as u8)));
    }

    #[test]
    fn test_jsr_resolves_to_function() {
        let ncs = compile_to_ncs("void foo() { } void main() { foo(); }");
        // Find JSR instruction and verify its target is non-zero
        for i in 0..ncs.len()-5 {
            if ncs[i] == Opcode::Jsr as u8 && ncs[i+1] == 0 {
                let offset = i32::from_be_bytes([ncs[i+2], ncs[i+3], ncs[i+4], ncs[i+5]]);
                assert_ne!(offset, 0, "JSR offset should be resolved, not zero");
            }
        }
    }

    #[test]
    fn test_global_variables() {
        let ncs = compile_to_ncs("int gCount = 10;\nvoid main() { }");
        // Should have SAVE_BASE_POINTER and RESTORE_BASE_POINTER
        assert!(ncs.contains(&(Opcode::SaveBasePointer as u8)));
        assert!(ncs.contains(&(Opcode::RestoreBasePointer as u8)));
    }

    #[test]
    fn test_boolean_not() {
        let ncs = compile_to_ncs("void main() { int x = !0; }");
        assert!(ncs.contains(&(Opcode::BooleanNot as u8)));
    }

    #[test]
    fn test_negation() {
        let ncs = compile_to_ncs("void main() { int x = -42; }");
        assert!(ncs.contains(&(Opcode::Negation as u8)));
    }

    #[test]
    fn test_do_while() {
        let ncs = compile_to_ncs("void main() { int x = 0; do { x = x + 1; } while (x < 10); }");
        assert!(ncs.contains(&(Opcode::Jnz as u8)));
    }

    #[test]
    fn test_logical_and_short_circuit() {
        let ncs = compile_to_ncs("void main() { int x = 1 && 0; }");
        assert!(ncs.contains(&(Opcode::Jz as u8)));
    }

    #[test]
    fn test_comparison() {
        let ncs = compile_to_ncs("void main() { int x = (1 > 2) ? 1 : 0; }");
        assert!(ncs.contains(&(Opcode::GT as u8)));
    }

    #[test]
    fn test_execute_command_for_engine_func() {
        let spec = "void PrintString(string s);";
        let mut lexer = Lexer::new("void main() { PrintString(\"hi\"); }", "test.nss", 0);
        let tokens = lexer.tokenize();
        let mut parser = Parser::new(tokens);
        parser.file_names.push("test.nss".to_string());
        let root = parser.parse_program().unwrap();

        let mut checker = SemanticChecker::new(&parser.arena, &parser.file_names);
        checker.set_require_entry_point(false);
        checker.load_lang_spec(spec);
        let _ = checker.check(root);

        let mut cg = CodeGenerator::new(&parser.arena, &parser.file_names);
        cg.load_symbols(&checker);
        let ncs = cg.generate(root).unwrap();

        assert!(ncs.contains(&(Opcode::ExecuteCommand as u8)));
    }

    #[test]
    fn test_vector_constant() {
        let ncs = compile_to_ncs("void main() { vector v = Vector(1.0, 2.0, 3.0); }");
        let float_consts = ncs.windows(2)
            .filter(|w| w[0] == Opcode::Constant as u8 && w[1] == 0x04)
            .count();
        assert!(float_consts >= 3, "Expected >= 3 float constants, got {}", float_consts);
    }

    #[test]
    fn test_deterministic_50x() {
        let src = "void main() { int a = 0; while (a < 10) { a = a + 1; } }";
        let first = compile_to_ncs(src);
        for _ in 0..49 {
            assert_eq!(compile_to_ncs(src), first);
        }
    }
}
