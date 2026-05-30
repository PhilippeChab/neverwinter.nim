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
    // True for variables in the global area. For globals, `stack_offset` is the
    // byte position within the global area (0 = first global). Address as
    // (stack_offset - global_var_size) relative to BP.
    is_global: bool,
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
    // Compile-time values of `const` declarations (main file + includes), imported
    // from the semantic checker. C++ folds a const reference to its literal value at
    // its use site (consts allocate no runtime storage); we emit the literal here.
    const_values: std::collections::HashMap<String, crate::semcheck::DefaultValue>,
    current_func_name: Option<String>,
    current_return_type: NwType,
    current_return_type_name: Option<String>,
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
    // Stack depth and code-offset at the entry of each enclosing loop/switch.
    // Code-offset is used to pick the innermost scope unambiguously (C++ compares
    // m_nSwitchIdentifier vs m_nLoopIdentifier, both being code offsets).
    loop_entry_depth_stack: Vec<i32>,
    loop_entry_offset_stack: Vec<usize>,
    switch_entry_depth_stack: Vec<i32>,
    switch_entry_offset_stack: Vec<usize>,
    has_globals: bool,
    global_var_size: i32,
    // Recursion-depth guard for expression codegen (defense-in-depth: semcheck
    // already caps at 2000, but a deep operator chain shouldn't crash codegen either).
    expr_depth: u32,
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
            const_values: std::collections::HashMap::new(),
            current_func_name: None,
            current_return_type: NwType::Void,
            current_return_type_name: None,
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
            loop_entry_depth_stack: Vec::new(),
            loop_entry_offset_stack: Vec::new(),
            switch_entry_depth_stack: Vec::new(),
            switch_entry_offset_stack: Vec::new(),
            has_globals: false,
            global_var_size: 0,
            expr_depth: 0,
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

        // Import const values so a const reference can be folded to its literal at the
        // use site (C++ behavior — consts allocate no storage). The checker has already
        // resolved every const (main file + includes), including const-from-const.
        self.const_values = checker.collect_const_values();

        // C++ scriptcompcore.cpp:925-950 seeds the struct table with the "vector"
        // pseudo-struct, then user-defined structs follow. The NDB writer emits
        // one `s` / `sf` block per entry.
        self.ndb_structs.clear();
        self.ndb_structs.push(crate::ndb::NdbStructDef {
            name: "vector".to_string(),
            fields: vec![
                ("x".to_string(), crate::types::NwType::Float, String::new()),
                ("y".to_string(), crate::types::NwType::Float, String::new()),
                ("z".to_string(), crate::types::NwType::Float, String::new()),
            ],
        });
        for s in &self.struct_defs {
            if s.name == "vector" { continue; }
            self.ndb_structs.push(crate::ndb::NdbStructDef {
                name: s.name.clone(),
                fields: s.fields.iter().map(|f| (
                    f.name.clone(),
                    f.nw_type,
                    f.type_name.clone().unwrap_or_default(),
                )).collect(),
            });
        }
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

    fn find_var(&self, name: &str) -> Option<(i32, i32, NwType, Option<String>, bool)> {
        self.locals.iter().rev().find(|l| l.name == name)
            .map(|l| (l.stack_offset, l.size, l.nw_type, l.type_name.clone(), l.is_global))
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

    /// Resolve the struct type-name of an expression node (the parser does not populate
    /// `type_name` on Variable/Action nodes). Mirrors gen_struct_field_read_typed.
    fn resolve_expr_struct_name(&self, node_id: NodeId) -> Option<String> {
        if node_id == NULL_NODE { return None; }
        let n = self.arena.get(node_id).clone();
        match n.op {
            Operation::Variable => {
                let name = n.string_data.as_deref()?;
                self.find_var(name).and_then(|(_, _, _, tn, _)| tn)
            }
            Operation::Action => {
                if n.left == NULL_NODE { return None; }
                let aid = self.arena.get(n.left);
                let fname = aid.string_data.as_deref()?;
                self.func_sigs.iter().find(|f| f.name == fname)
                    .and_then(|f| f.return_type_name.clone())
            }
            Operation::StructurePart => {
                // The field's struct type, resolved from its parent struct.
                let field = n.string_data.as_deref()?;
                let parent = self.resolve_expr_struct_name(n.left)?;
                self.struct_defs.iter().find(|s| s.name == parent)
                    .and_then(|s| s.fields.iter().find(|f| f.name == field))
                    .and_then(|f| f.type_name.clone())
            }
            Operation::CondBlock => {
                // Ternary: resolve the then-branch (CondBlock.right is the CondChoice
                // whose .left is the then-branch). Matches the semcheck resolver so a
                // struct ternary used as a `==` operand emits the correct size operand.
                if n.right == NULL_NODE { return None; }
                let choice_left = self.arena.get(n.right).left;
                self.resolve_expr_struct_name(choice_left)
            }
            _ => n.type_name.clone(),
        }
    }

    /// Walks a nested `s.a.b.c` StructurePart chain down to the root Variable and
    /// returns (combined_offset, field_size, is_global). Used by inc/dec and could
    /// be reused for struct-field assignment.
    fn resolve_struct_field_lvalue(&self, node_id: NodeId) -> Option<(i32, i32, bool)> {
        let mut path: Vec<String> = Vec::new();
        let mut cur = node_id;
        loop {
            let n = self.arena.get(cur).clone();
            match n.op {
                Operation::StructurePart => {
                    path.push(n.string_data.as_deref().unwrap_or("").to_string());
                    if n.left == NULL_NODE { return None; }
                    cur = n.left;
                }
                Operation::Variable => break,
                _ => return None,
            }
        }
        let root = self.arena.get(cur).clone();
        let var_name = root.string_data.as_deref()?;
        let (so, _struct_sz, _, type_name, is_global) = self.find_var(var_name)?;
        let mut cur_type_name = type_name;
        let mut cur_offset = 0i32;
        let mut cur_size = 4i32;
        for field in path.iter().rev() {
            if let Some(tn) = &cur_type_name {
                let (off, sz, ftype) = self.struct_field_offset(tn, field)?;
                cur_offset += off;
                cur_size = sz;
                cur_type_name = if ftype == NwType::Struct {
                    self.struct_defs.iter()
                        .find(|s| s.name == *tn)
                        .and_then(|s| s.fields.iter().find(|f| f.name == *field))
                        .and_then(|f| f.type_name.clone())
                } else {
                    None
                };
            } else {
                let (off, sz) = match field.as_str() {
                    "x" => (0, 4),
                    "y" => (4, 4),
                    "z" => (8, 4),
                    _ => return None,
                };
                cur_offset += off;
                cur_size = sz;
            }
        }
        Some((so + cur_offset, cur_size, is_global))
    }

    fn struct_field_offset(&self, struct_name: &str, field_name: &str) -> Option<(i32, i32, NwType)> {
        if let Some(sd) = self.struct_defs.iter().find(|s| s.name == struct_name) {
            for f in &sd.fields {
                if f.name == field_name {
                    // C++ scriptcompfinalcode.cpp:4830-4849 uses the FIELD'S type-name
                    // to compute size — for a struct-typed field it's the inner struct's
                    // full byte size, not the scalar size (which is 0 for Struct).
                    let sz = self.type_size(f.nw_type, &f.type_name);
                    return Some((f.offset, sz, f.nw_type));
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
        // Iterate the flat FunctionalUnit chain (per-declaration recursion overflows
        // the stack on large includes like nwscript.nss).
        let mut cur = node_id;
        while cur != NULL_NODE {
            let node = self.arena.get(cur);
            match node.op {
                Operation::FunctionalUnit => {
                    if node.left != NULL_NODE
                        && self.arena.get(node.left).op == Operation::GlobalVariables
                    {
                        return true;
                    }
                    cur = node.right;
                }
                Operation::GlobalVariables => return true,
                _ => return false,
            }
        }
        false
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
        // C++: loader is `JSR (#globals|entry); RET`.
        // SAVE/RESTORE_BASE_POINTER live inside #globals.
        // For StartingConditional WITHOUT globals, C++ scriptcompfinalcode.cpp:572-600
        // also reserves a return-value slot before the JSR so the conditional's
        // bytecode has somewhere to write its result.
        let loader_start = self.pos();
        let entry = self.entry_point_name().to_string();
        let is_conditional = entry == "StartingConditional";
        // C++ scriptcompfinalcode.cpp:571-599 (InstallLoader): for an int-returning
        // conditional the loader emits RUNSTACK_ADD INTEGER before the JSR
        // UNCONDITIONALLY. has_globals only picks the JSR target (#globals vs entry),
        // not whether the retval slot is reserved. The #globals writeback offset
        // `-(global_var_size + 12)` assumes this loader slot exists.
        if is_conditional {
            self.emit_op(Opcode::RunstackAdd, NwType::Integer.auxcode());
            self.stack_depth += 1;
        }
        if self.has_globals {
            self.emit_jsr_label("#globals");
        } else {
            self.emit_jsr_label(&entry);
        }
        self.emit_op(Opcode::Ret, 0);
        // C++ registers #loader as a real identifier and emits an NDB function entry
        // for it (scriptcompfinalcode.cpp:556-648, NDB loop :6938).
        self.ndb_functions.push(crate::ndb::NdbFunctionEntry {
            name: "#loader".to_string(),
            return_type: NwType::Void,
            return_struct_name: String::new(),
            code_start: loader_start as u32,
            code_end: self.pos() as u32,
            params: Vec::new(),
        });
        Ok(())
    }

    fn emit_globals_func(&mut self, root: NodeId) -> Result<(), CompileError> {
        let globals_start = self.pos();
        self.add_label("#globals");
        self.stack_depth = 0;
        self.global_var_size = 0;

        // Walk the tree to emit global variable initializers
        self.emit_global_var_inits(root)?;

        // After globals are initialized, call entry point.
        let entry = self.entry_point_name().to_string();
        let is_conditional = entry == "StartingConditional";

        // C++ ordering for conditional scripts:
        //   SAVE_BASE_POINTER
        //   RUNSTACK_ADD INTEGER         ; retval slot
        //   JSR entry
        //   ASSIGNMENT -(globals+12), 4  ; writeback retval into globals area
        //   MODIFY_STACK_POINTER -4
        //   RESTORE_BASE_POINTER
        self.emit_op(Opcode::SaveBasePointer, 0);
        if is_conditional {
            self.emit_op(Opcode::RunstackAdd, NwType::Integer.auxcode());
            self.stack_depth += 1;
        }
        self.emit_jsr_label(&entry);
        if is_conditional {
            // Writeback offset = -(global_var_size + 12) per C++
            self.emit_op(Opcode::Assignment, 0x01);
            self.emit_i32(-(self.global_var_size + 12));
            self.emit_u16(4);
            self.emit_modify_sp(-4);
            self.stack_depth -= 1;
        }
        self.emit_op(Opcode::RestoreBasePointer, 0);

        // Clean up globals from stack
        if self.global_var_size > 0 {
            self.emit_modify_sp(-self.global_var_size);
        }

        self.emit_op(Opcode::Ret, 0);

        // C++ emits an NDB function entry for #globals (scriptcompfinalcode.cpp:5196).
        self.ndb_functions.push(crate::ndb::NdbFunctionEntry {
            name: "#globals".to_string(),
            return_type: NwType::Void,
            return_struct_name: String::new(),
            code_start: globals_start as u32,
            code_end: self.pos() as u32,
            params: Vec::new(),
        });
        Ok(())
    }

    fn emit_global_var_inits(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        if node_id == NULL_NODE { return Ok(()); }
        let node = self.arena.get(node_id).clone();
        match node.op {
            Operation::FunctionalUnit => {
                // Iterate the flat right-linked chain; recurse only into each FU's
                // (bounded-depth) left declaration. Per-declaration recursion on the
                // chain overflows the stack on large includes (nwscript.nss).
                let mut cur = node_id;
                while cur != NULL_NODE {
                    let n = self.arena.get(cur).clone();
                    if n.op != Operation::FunctionalUnit { break; }
                    self.emit_global_var_inits(n.left)?;
                    cur = n.right;
                }
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

                        // Record byte position within the global area BEFORE pushing,
                        // so it corresponds to the first byte of this variable.
                        let global_byte_offset = self.global_var_size;

                        if var.left != NULL_NODE {
                            self.generate_expr(var.left)?;
                        } else {
                            self.emit_default_value_named(nw_type, &type_name);
                        }

                        // C++ emits an NDB variable entry for each global (resolved
                        // within #globals' code range, scriptcompfinalcode.cpp:6709-6744).
                        let code_pos = self.pos() as u32;
                        self.ndb_variables.push(crate::ndb::NdbVarEntry {
                            name: name.clone(),
                            var_type: nw_type,
                            struct_name: type_name.clone().unwrap_or_default(),
                            stack_loc: global_byte_offset as u32,
                            code_start: code_pos,
                            code_end: code_pos,
                        });

                        self.locals.push(LocalVar {
                            name, nw_type, type_name: type_name.clone(),
                            stack_offset: global_byte_offset,
                            scope_level: 0, size,
                            is_global: true,
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
        self.emit_default_value_named(nw_type, &None);
    }

    /// C++ `AddVariableToStack` (scriptcompfinalcode.cpp:6011-6062) emits one
    /// `RUNSTACK_ADD <type-auxcode>` per uninitialized scalar — not a CONSTI 0.
    /// Structs recurse over their fields; engine structs use their own auxcode.
    fn emit_default_value_named(&mut self, nw_type: NwType, type_name: &Option<String>) {
        self.emit_default_value_named_guarded(nw_type, type_name, &mut Vec::new());
    }

    fn emit_default_value_named_guarded(
        &mut self,
        nw_type: NwType,
        type_name: &Option<String>,
        active: &mut Vec<String>,
    ) {
        match nw_type {
            NwType::Integer => {
                self.emit_op(Opcode::RunstackAdd, NwType::Integer.auxcode());
                self.stack_depth += 1;
            }
            NwType::Float => {
                self.emit_op(Opcode::RunstackAdd, NwType::Float.auxcode());
                self.stack_depth += 1;
            }
            NwType::String => {
                self.emit_op(Opcode::RunstackAdd, NwType::String.auxcode());
                self.stack_depth += 1;
            }
            NwType::Object => {
                self.emit_op(Opcode::RunstackAdd, NwType::Object.auxcode());
                self.stack_depth += 1;
            }
            NwType::Vector => {
                self.emit_op(Opcode::RunstackAdd, NwType::Float.auxcode());
                self.emit_op(Opcode::RunstackAdd, NwType::Float.auxcode());
                self.emit_op(Opcode::RunstackAdd, NwType::Float.auxcode());
                self.stack_depth += 3;
            }
            NwType::EngineStructure(_) => {
                self.emit_op(Opcode::RunstackAdd, nw_type.auxcode());
                self.stack_depth += 1;
            }
            NwType::Struct => {
                if let Some(tn) = type_name {
                    // Anti-crash depth cap: a legitimately deep ACYCLIC struct chain
                    // (`struct S0{struct S1 f;} … struct SN{int f;}`) never trips the
                    // cycle guard below, so without a depth bound it overflows the fixed
                    // WASM stack (~7000 frames). Cap well below that; no real struct
                    // nests anywhere near this deep.
                    if active.len() >= 1000 {
                        return;
                    }
                    // Break cycles in the struct type graph. C++ never builds such a
                    // cycle (its lexer rejects the forward struct reference at parse
                    // time); our more-lenient parser can, so guard against unbounded
                    // recursion on `struct A{struct B b;} struct B{struct A a;}`.
                    if active.iter().any(|n| n == tn) {
                        return;
                    }
                    let sd = self.struct_defs.iter().find(|s| s.name == *tn).cloned();
                    if let Some(sd) = sd {
                        active.push(tn.clone());
                        for f in &sd.fields {
                            self.emit_default_value_named_guarded(f.nw_type, &f.type_name, active);
                        }
                        active.pop();
                    }
                }
            }
            NwType::Void | NwType::Action => {
                self.emit_op(Opcode::RunstackAdd, NwType::Integer.auxcode());
                self.stack_depth += 1;
            }
        }
    }

    fn emit_all_functions(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        if node_id == NULL_NODE { return Ok(()); }
        let node = self.arena.get(node_id).clone();
        if node.op == Operation::FunctionalUnit {
            // Iterate the flat chain; recurse only into each FU's bounded-depth left.
            let mut cur = node_id;
            while cur != NULL_NODE {
                let n = self.arena.get(cur).clone();
                if n.op != Operation::FunctionalUnit { break; }
                self.emit_all_functions(n.left)?;
                cur = n.right;
            }
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
        // Iterate the flat FunctionalUnit chain; recurse only into each bounded left.
        let mut cur = node_id;
        while cur != NULL_NODE {
            let node = self.arena.get(cur);
            if node.op == Operation::FunctionalUnit {
                let left = node.left;
                let right = node.right;
                if let Some(b) = self.find_function_body(left, name) { return Some(b); }
                cur = right;
                continue;
            }
            if node.op == Operation::Function && node.left != NULL_NODE {
                let fid = self.arena.get(node.left);
                if fid.string_data.as_deref() == Some(name) {
                    return Some(node.right);
                }
            }
            break;
        }
        None
    }

    fn collect_calls(&self, node_id: NodeId, out: &mut Vec<String>) {
        // Explicit-stack walk over a whole function body (recursion overflows on
        // large bodies). Order doesn't matter — `out` is a reachability set.
        let mut stack = vec![node_id];
        while let Some(id) = stack.pop() {
            if id == NULL_NODE { continue; }
            let node = self.arena.get(id);
            if node.op == Operation::Action && node.left != NULL_NODE {
                let aid = self.arena.get(node.left);
                if let Some(n) = &aid.string_data {
                    // Only collect user functions, not engine actions
                    if self.func_sigs.iter().any(|f| f.name == *n && !f.is_engine_action) {
                        out.push(n.clone());
                    }
                }
            }
            if node.right != NULL_NODE { stack.push(node.right); }
            if node.left != NULL_NODE { stack.push(node.left); }
        }
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
        self.current_return_type_name = fid.type_name.clone();
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
                is_global: false,
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

        // Skip the trailing RET if the body's last emitted instruction already is a
        // RET — but only when the RET was emitted INSIDE this function's code range.
        // (Otherwise an empty function placed after a function that ends in RET would
        // see the previous function's RET and emit nothing for itself.)
        let pos = self.pos();
        let already_ret = pos >= self.current_func_start + 2
            && self.code[pos - 2] == Opcode::Ret as u8
            && self.code[pos - 1] == 0;
        if !already_ret {
            self.emit_op(Opcode::Ret, 0);
        }

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
                // Iterate the right-linked statement chain; recurse only into each
                // statement (node.left). A 2000+-statement function body overflows
                // the stack if the chain is walked recursively.
                let mut cur = node_id;
                while cur != NULL_NODE {
                    let n = self.arena.get(cur).clone();
                    if n.op != Operation::StatementList {
                        // Not a list link — emit it as a single statement and stop.
                        self.generate_stmt(cur)?;
                        break;
                    }
                    self.generate_stmt(n.left)?;
                    cur = n.right;
                }
            }
            Operation::Statement | Operation::StatementNoDebug => {
                let line_start = self.pos();
                let line = node.line;
                let file_id = node.file_id;
                // C++ STATEMENT InVisit (scriptcompfinalcode.cpp:1449) records the
                // SP before, then the PostVisit (2240-2267) emits MODIFY_STACK_POINTER
                // to drop any unused expression value — EXCEPT when the inner op is
                // a KEYWORD_DECLARATION / CONST_DECLARATION (locals stay live until
                // their enclosing CompoundStatement closes).
                let saved_depth = self.stack_depth;
                let inner_is_decl = node.left != NULL_NODE && {
                    let inner = self.arena.get(node.left);
                    matches!(inner.op, Operation::KeywordDeclaration | Operation::ConstDeclaration)
                };
                self.generate_stmt(node.left)?;
                if !inner_is_decl {
                    let delta = (self.stack_depth - saved_depth) * 4;
                    if delta > 0 {
                        self.emit_modify_sp(-delta);
                    }
                }
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
            // A plain `while` body reaches gen_while as the WhileChoice wrapper
            // (extract_for_update only unwraps the for-desugar form). Emit its body.
            Operation::WhileChoice => self.generate_stmt(node.left)?,
            Operation::SwitchBlock => self.gen_switch(node_id)?,
            Operation::Return => self.gen_return(node_id)?,
            Operation::Break => {
                // C++ scriptcompfinalcode.cpp:5715 picks the innermost enclosing
                // loop or switch via code-offset comparison (whichever was entered
                // later, i.e. has the larger code offset, is innermost).
                let s_off = self.switch_entry_offset_stack.last().copied();
                let l_off = self.loop_entry_offset_stack.last().copied();
                let s_depth = self.switch_entry_depth_stack.last().copied();
                let l_depth = self.loop_entry_depth_stack.last().copied();
                let target_depth = match (s_off, l_off) {
                    (Some(so), Some(lo)) => {
                        if so >= lo { s_depth } else { l_depth }
                    }
                    (Some(_), None) => s_depth,
                    (None, Some(_)) => l_depth,
                    (None, None) => None,
                };
                if let Some(td) = target_depth {
                    let delta = (self.stack_depth - td) * 4;
                    if delta > 0 {
                        self.emit_modify_sp(-delta);
                    }
                }
                let fix = self.emit_jmp_placeholder(Opcode::Jmp);
                if let Some(exits) = self.break_fixup_stack.last_mut() {
                    exits.push(fix);
                }
            }
            Operation::Continue => {
                // C++: same SP rollback for continue inside loops.
                if let Some(&td) = self.loop_entry_depth_stack.last() {
                    let delta = (self.stack_depth - td) * 4;
                    if delta > 0 {
                        self.emit_modify_sp(-delta);
                    }
                }
                if let Some(&target) = self.loop_start_stack.last() {
                    if target == usize::MAX {
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
                // C++ scriptcompfinalcode.cpp:2818 / 5293 / 5404 sets
                // m_nVarRunTimeLocation = m_nStackCurrentDepth * 4 BEFORE the value
                // is pushed. Reading uses `nIntegerData - m_nStackCurrentDepth*4`,
                // so the first local reads at offset -4 (below SP). Capture the
                // offset BEFORE the push to match.
                let offset = self.stack_depth * 4;
                if var.left != NULL_NODE {
                    self.generate_expr(var.left)?;
                } else {
                    self.emit_default_value_named(nw_type, &type_name);
                }

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
                    is_global: false,
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
        self.loop_entry_depth_stack.push(self.stack_depth);
        self.loop_entry_offset_stack.push(self.pos());
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
            // C++ wraps the for-loop UPDATE in a STATEMENT node whose post-visit
            // (scriptcompfinalcode.cpp:2256-2267) emits MODIFY_STACK_POINTER to drop
            // any residue from the update expression (e.g. an int-returning call).
            // Rust parses the update bare, so emit the same cleanup here.
            let upd = self.arena.get(update_node).clone();
            if upd.left != NULL_NODE {
                let saved_depth = self.stack_depth;
                self.generate_expr(upd.left)?;
                let delta = (self.stack_depth - saved_depth) * 4;
                if delta > 0 {
                    self.emit_modify_sp(-delta);
                }
            }
        }

        self.emit_jmp_to(Opcode::Jmp, loop_top);
        self.patch_jmp_here(jz);

        for f in self.break_fixup_stack.pop().unwrap_or_default() { self.patch_jmp_here(f); }
        if continue_fixups_idx.is_some() {
            self.continue_fixup_stack.pop();
        }
        self.loop_start_stack.pop();
        self.loop_entry_depth_stack.pop();
        self.loop_entry_offset_stack.pop();
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
        self.loop_entry_depth_stack.push(self.stack_depth);
        self.loop_entry_offset_stack.push(self.pos());
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
        self.loop_entry_depth_stack.pop();
        self.loop_entry_offset_stack.pop();
        Ok(())
    }

    fn gen_switch(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();

        if node.left != NULL_NODE {
            let cond = self.arena.get(node.left).clone();
            if cond.left != NULL_NODE { self.generate_expr(cond.left)?; }
        }

        // C++ scriptcompfinalcode.cpp:5748-5752: break inside switch rolls SP to
        // (m_nSwitchStackDepth + 1) — i.e. the depth WITH the switch expression
        // on the stack. Record entry depth AFTER pushing the expression so a break
        // leaves the expression alone; the final MODIFY_SP at switch exit pops it.
        let entry_depth = self.stack_depth;
        self.switch_entry_depth_stack.push(entry_depth);
        self.switch_entry_offset_stack.push(self.pos());
        self.break_fixup_stack.push(Vec::new());

        self.gen_switch_body(node.right)?;

        // Patch break fixups to point HERE — break jumps to the same single MODIFY_SP
        // that handles the natural exit, popping the switch expression exactly once.
        for f in self.break_fixup_stack.pop().unwrap_or_default() { self.patch_jmp_here(f); }
        self.emit_modify_sp(-4); // pop switch expression
        self.switch_entry_depth_stack.pop();
        self.switch_entry_offset_stack.pop();
        Ok(())
    }

    fn gen_switch_body(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        // Two-pass approach matching C++ (scriptcompfinalcode.cpp `GenerateCodeForSwitchLabels`):
        // 1. Pre-emit the dispatch table — for each case label, COPYTOP + CONST + EQUAL + JNZ → label
        //    Followed by a final JMP to the default label, or past the switch if none.
        // 2. Emit the body. CASE / Default nodes become pure labels (no extra code).
        let mut items = Vec::new();
        self.flatten_switch_items(node_id, &mut items);

        // Pass 1: dispatch.
        // For each case we record (item_index_in_items, jnz_fixup).
        let mut case_jumps: Vec<(usize, usize)> = Vec::new();
        let mut default_jump: Option<(usize, usize)> = None; // (item_index, jmp_fixup)
        for (i, &item_id) in items.iter().enumerate() {
            let item = self.arena.get(item_id).clone();
            match item.op {
                Operation::Case => {
                    // dup switch expression
                    self.emit_op(Opcode::RunstackCopy, 0x01);
                    self.emit_i32(-4); self.emit_u16(4);
                    self.stack_depth += 1;
                    if item.left != NULL_NODE {
                        let case_val = self.arena.get(item.left).clone();
                        if case_val.op == Operation::ConstantString {
                            let s = case_val.string_data.as_deref().unwrap_or("");
                            let h = crate::xxh32::cexo_string_hash(s);
                            self.emit_const_int(h);
                        } else {
                            self.generate_expr(item.left)?;
                        }
                    }
                    self.emit_op(Opcode::Equal, 0x20);
                    self.stack_depth -= 1;
                    let jnz = self.emit_jmp_placeholder(Opcode::Jnz);
                    self.stack_depth -= 1;
                    case_jumps.push((i, jnz));
                }
                Operation::Default => {
                    // Defer — emit final JMP to default once we've finished the dispatch table.
                    default_jump = Some((i, usize::MAX));
                }
                _ => {}
            }
        }
        // Final JMP at the end of dispatch — to default (if present) or past the switch body.
        let final_jmp_fixup = self.emit_jmp_placeholder(Opcode::Jmp);

        // Pass 2: body. Each Case / Default acts as a label — patch the corresponding jump here.
        for (i, &item_id) in items.iter().enumerate() {
            let item = self.arena.get(item_id).clone();
            match item.op {
                Operation::Case => {
                    // Find this case's pre-emitted JNZ and patch it to here.
                    if let Some(&(_, jnz)) = case_jumps.iter().find(|(idx, _)| *idx == i) {
                        self.patch_jmp_here(jnz);
                    }
                }
                Operation::Default => {
                    if let Some((idx, _)) = default_jump {
                        if idx == i {
                            // Patch the final dispatch JMP to here (the default label).
                            self.patch_jmp_here(final_jmp_fixup);
                            default_jump = Some((idx, 0)); // mark patched
                        }
                    }
                }
                _ => {
                    self.generate_stmt(item_id)?;
                }
            }
        }

        // If there was no default, the dispatch JMP falls through to switch exit.
        if let Some((_, 0)) = default_jump {
            // already patched at the Default label
        } else if default_jump.is_none() {
            self.patch_jmp_here(final_jmp_fixup);
        }

        Ok(())
    }

    fn flatten_switch_items(&self, node_id: NodeId, items: &mut Vec<NodeId>) {
        // Explicit-stack in-order flatten (recursion overflows on ~500+ case bodies).
        // Push right then left so left is emitted first, preserving source order.
        let mut stack = vec![node_id];
        while let Some(id) = stack.pop() {
            if id == NULL_NODE { continue; }
            let node = self.arena.get(id);
            if node.op == Operation::StatementList {
                stack.push(node.right);
                stack.push(node.left);
            } else {
                items.push(id);
            }
        }
    }

    fn gen_return(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        if node.left != NULL_NODE {
            let ret_type_name = self.current_return_type_name.clone();
            let ret_size = self.type_size(self.current_return_type, &ret_type_name);
            self.generate_expr(node.left)?;

            // Copy return value to caller's reserved slot.
            if ret_size > 0 {
                let target_offset = self.current_return_slot_offset - self.stack_depth * 4;
                self.emit_op(Opcode::Assignment, 0x01);
                self.emit_i32(target_offset);
                self.emit_u16(ret_size as u16);
            }
        }
        // C++ scriptcompfinalcode.cpp:5469-5503 emits ONE MODIFY_STACK_POINTER that
        // pops the return value AND every leftover local in a single instruction.
        let pop_bytes = (self.stack_depth - self.base_stack_depth) * 4;
        if pop_bytes > 0 {
            self.emit_modify_sp(-pop_bytes);
        }
        self.emit_op(Opcode::Ret, 0);
        Ok(())
    }

    // ========== Expression codegen ==========

    fn generate_expr(&mut self, node_id: NodeId) -> Result<NwType, CompileError> {
        if node_id == NULL_NODE { return Ok(NwType::Void); }
        // Defense-in-depth stack guard (semcheck already caps deep operator chains).
        self.expr_depth += 1;
        if self.expr_depth > 2000 {
            self.expr_depth -= 1;
            return Err(CompileError::UnexpectedCharacter);
        }
        let r = self.generate_expr_inner(node_id);
        self.expr_depth -= 1;
        r
    }

    fn generate_expr_inner(&mut self, node_id: NodeId) -> Result<NwType, CompileError> {
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
                if let Some((so, sz, nt, _tn, is_global)) = self.find_var(name) {
                    if is_global {
                        let bp_offset = so - self.global_var_size;
                        self.emit_op(Opcode::RunstackCopyBase, 0x01);
                        self.emit_i32(bp_offset);
                        self.emit_u16(sz as u16);
                    } else {
                        let offset = so - self.stack_depth * 4;
                        self.emit_op(Opcode::RunstackCopy, 0x01);
                        self.emit_i32(offset);
                        self.emit_u16(sz as u16);
                    }
                    self.stack_depth += sz / 4;
                    Ok(nt)
                } else if let Some(dv) = self.const_values.get(name).cloned() {
                    // Not a local/global variable, but a `const` — fold to its literal
                    // value (C++ inlines const references; consts have no storage).
                    use crate::semcheck::DefaultValue as DV;
                    let t = match &dv {
                        DV::Integer(_) => NwType::Integer,
                        DV::Float(_) => NwType::Float,
                        DV::String(_) => NwType::String,
                        DV::Object(_) => NwType::Object,
                        DV::Vector(_, _, _) => NwType::Vector,
                        DV::EngineStruct => NwType::Void,
                    };
                    self.emit_default_value_from(&dv);
                    Ok(t)
                } else {
                    Ok(NwType::Void)
                }
            }

            // ---- Assignment ----
            Operation::Assignment => {
                let op_token = node.int_data[0];
                let is_compound = op_token != crate::token::TokenType::AssignmentEqual as i32
                    && op_token != 0;

                let mut lhs_type = NwType::Void;
                if is_compound && node.left != NULL_NODE {
                    // Compound assignment (+=, -=, etc.): load current value first
                    lhs_type = self.generate_expr(node.left)?;
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
                        // C++: auxcode is derived from operand types, not hardcoded
                        let aux = lhs_type.auxcode_pair(rt).unwrap_or(0x20);
                        self.emit_op(op, aux);
                        self.stack_depth -= 1;
                    }
                }

                if node.left != NULL_NODE {
                    let lhs = self.arena.get(node.left).clone();
                    if lhs.op == Operation::Variable {
                        let name = lhs.string_data.as_deref().unwrap_or("");
                        if let Some((so, sz, _, _, is_global)) = self.find_var(name) {
                            if is_global {
                                let bp_offset = so - self.global_var_size;
                                self.emit_op(Opcode::AssignmentBase, 0x01);
                                self.emit_i32(bp_offset);
                                self.emit_u16(sz as u16);
                            } else {
                                let offset = so - self.stack_depth * 4;
                                self.emit_op(Opcode::Assignment, 0x01);
                                self.emit_i32(offset);
                                self.emit_u16(sz as u16);
                            }
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
                // Stack accounting per C++ scriptcompfinalcode.cpp:4503-4591:
                //   vec+vec: pop 6 slots, push 3 → net -3
                //   vec*float / float*vec: pop 4 slots, push 3 → net -1
                //   string+string / int+int / float+float etc: pop 2 slots, push 1 → net -1
                let (result_type, sp_delta) = match (lt, rt) {
                    (NwType::Vector, NwType::Vector) => (NwType::Vector, -3),
                    (NwType::Vector, NwType::Float) | (NwType::Float, NwType::Vector) => (NwType::Vector, -1),
                    (NwType::Float, _) | (_, NwType::Float) => (NwType::Float, -1),
                    (NwType::String, NwType::String) if node.op == Operation::Add => (NwType::String, -1),
                    _ => (NwType::Integer, -1),
                };
                self.stack_depth += sp_delta;
                Ok(result_type)
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

            // ---- Logical AND / OR ----
            // C++ scriptcompfinalcode.cpp:2412-2466 + 3932-4010 emits BOTH a short-circuit
            // (RUNSTACK_COPY top + JZ/JNZ past the right operand and the opcode) AND a
            // final LogicalAnd / LogicalOr opcode. When the short-circuit fires, the
            // result is the (already-on-stack) left value; when it doesn't, LogicalAnd
            // / LogicalOr consumes both operands and pushes the normalised result.
            Operation::LogicalAnd => {
                self.generate_expr(node.left)?;
                // Copy top, JZ-skip if zero: keeps the left value on stack.
                self.emit_op(Opcode::RunstackCopy, 0x01);
                self.emit_i32(-4);
                self.emit_u16(4);
                self.stack_depth += 1;
                let jz = self.emit_jmp_placeholder(Opcode::Jz);
                self.stack_depth -= 1;
                self.generate_expr(node.right)?;
                self.emit_op(Opcode::LogicalAnd, 0x20);
                self.stack_depth -= 1;
                self.patch_jmp_here(jz);
                Ok(NwType::Integer)
            }
            Operation::LogicalOr => {
                // Mirror C++ scriptcompfinalcode.cpp (InVisit :2468, PostVisit :3971).
                // Unlike `&&`, the short-circuit value for `||` is the *truthy* left
                // operand, which must still be normalised to 1 through the LOGOR op —
                // so C++ does NOT skip the opcode. It copies the left value, and on the
                // truthy path makes a *second* copy and jumps straight to LOGOR (which
                // pops both copies and pushes `left||left` == 1). On the falsy path the
                // JZ pops the first copy and falls through to the right operand.
                self.generate_expr(node.left)?;
                // COPY top -> [left, left]
                self.emit_op(Opcode::RunstackCopy, 0x01);
                self.emit_i32(-4);
                self.emit_u16(4);
                self.stack_depth += 1;
                // JZ (pops the copy): left falsy -> evaluate the right operand.
                let jz = self.emit_jmp_placeholder(Opcode::Jz);
                self.stack_depth -= 1;
                // Truthy path: second copy (intentionally NOT tracked on the stack, per
                // C++) then jump over the right operand straight to LOGOR.
                self.emit_op(Opcode::RunstackCopy, 0x01);
                self.emit_i32(-4);
                self.emit_u16(4);
                let jmp = self.emit_jmp_placeholder(Opcode::Jmp);
                // Falsy path lands here, evaluates the right operand.
                self.patch_jmp_here(jz);
                self.generate_expr(node.right)?;
                // Both paths converge on LOGOR.
                self.patch_jmp_here(jmp);
                self.emit_op(Opcode::LogicalOr, 0x20);
                self.stack_depth -= 1;
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
                // C++ treats vector == / != as a STRUCT_STRUCT compare (aux 0x24, size 12)
                let mut aux = lt.auxcode_pair(rt).unwrap_or(0x20);
                let mut emit_size: Option<u16> = None;
                let mut pop_slots = 1i32;
                if matches!(node.op, Operation::ConditionEqual | Operation::ConditionNotEqual)
                    && lt == NwType::Vector && rt == NwType::Vector
                {
                    aux = 0x24;
                    emit_size = Some(12);
                    pop_slots = (12 / 4) * 2 - 1;
                } else if aux == 0x24 {
                    // C++ scriptcompfinalcode.cpp:4225-4263 emits GetStructureSize as the
                    // 2-byte size operand. The struct name must be resolved from the
                    // operand expression (a Variable's type_name is NOT set on the AST
                    // node — resolve via the symbol tables, like field reads do).
                    let name = self.resolve_expr_struct_name(node.left);
                    let sz = name.as_deref().map(|n| self.struct_size(n)).unwrap_or(0);
                    emit_size = Some(sz as u16);
                    pop_slots = (sz / 4) * 2 - 1;
                }
                self.emit_op(op, aux);
                if let Some(sz) = emit_size { self.emit_u16(sz); }
                self.stack_depth -= pop_slots;
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
                let mut result_slots = 1i32;
                if node.right != NULL_NODE {
                    let choice = self.arena.get(node.right).clone();
                    // C++ scriptcompfinalcode.cpp:2392-2398: between the two branches the
                    // compile-time stack is rolled back by the then-branch result's slot
                    // count (GetStructureSize for structs, 12 for vectors, 4 otherwise).
                    // Measure how many slots the then-branch actually pushed rather than
                    // relying on a possibly-unannotated AST type name.
                    let depth_before = self.stack_depth;
                    rt = self.generate_expr(choice.left)?;
                    result_slots = (self.stack_depth - depth_before).max(1);
                    let jmp = self.emit_jmp_placeholder(Opcode::Jmp);
                    self.stack_depth -= result_slots;
                    self.patch_jmp_here(jz);
                    self.generate_expr(choice.right)?;
                    self.patch_jmp_here(jmp);
                } else {
                    self.patch_jmp_here(jz);
                }
                let _ = result_slots;
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
            // ++/-- on a struct field: resolve the underlying variable + field offset.
            if lhs.op == Operation::StructurePart {
                if let Some((so, sz, is_global)) = self.resolve_struct_field_lvalue(node.left) {
                    let (read_op, mod_op, off_for_read, off_for_mod) = if is_global {
                        let base = so - self.global_var_size;
                        (Opcode::RunstackCopyBase,
                         match opcode {
                            Opcode::Increment => Opcode::IncrementBase,
                            Opcode::Decrement => Opcode::DecrementBase,
                            x => x,
                         },
                         base, base)
                    } else {
                        let sp_off = so - self.stack_depth * 4;
                        (Opcode::RunstackCopy, opcode, sp_off, so)
                    };
                    if !is_pre {
                        self.emit_op(read_op, 0x01);
                        self.emit_i32(off_for_read);
                        self.emit_u16(sz as u16);
                        self.stack_depth += sz / 4;
                    }
                    let mod_off = if is_global { off_for_mod } else { so - self.stack_depth * 4 };
                    self.emit_op(mod_op, 0x03);
                    self.emit_i32(mod_off);
                    if is_pre {
                        let post_off = if is_global { off_for_mod } else { so - self.stack_depth * 4 };
                        self.emit_op(read_op, 0x01);
                        self.emit_i32(post_off);
                        self.emit_u16(sz as u16);
                        self.stack_depth += sz / 4;
                    }
                    return Ok(NwType::Integer);
                }
            }
            if lhs.op == Operation::Variable {
                let name = lhs.string_data.as_deref().unwrap_or("");
                if let Some((so, sz, _, _, is_global)) = self.find_var(name) {
                    // For globals, use *Base opcodes with BP-relative offset.
                    let (read_op, mod_op, sp_off, bp_off) = if is_global {
                        let base = so - self.global_var_size;
                        (Opcode::RunstackCopyBase,
                         match opcode {
                            Opcode::Increment => Opcode::IncrementBase,
                            Opcode::Decrement => Opcode::DecrementBase,
                            x => x,
                         },
                         base, base)
                    } else {
                        (Opcode::RunstackCopy, opcode, so - self.stack_depth * 4, so)
                    };
                    if !is_pre {
                        // post: push old value first
                        let off = if is_global { bp_off } else { sp_off };
                        self.emit_op(read_op, 0x01);
                        self.emit_i32(off);
                        self.emit_u16(sz as u16);
                        self.stack_depth += sz / 4;
                    }
                    let mod_off = if is_global { bp_off } else { so - self.stack_depth * 4 };
                    self.emit_op(mod_op, 0x03);
                    self.emit_i32(mod_off);
                    if is_pre {
                        let off = if is_global { bp_off } else { so - self.stack_depth * 4 };
                        self.emit_op(read_op, 0x01);
                        self.emit_i32(off);
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

        // Resolve the function signature first so we can decide on calling convention.
        let engine_sig = self.find_engine_func(&func_name).map(|(id, s)| (id, s.clone()));
        let user_sig = self.find_user_func(&func_name).cloned();
        let sig_opt = engine_sig.as_ref().map(|(_, s)| s.clone()).or_else(|| user_sig.clone());
        let is_user = engine_sig.is_none() && user_sig.is_some();

        // For user functions, C++ reserves the return slot BEFORE arguments are pushed:
        //   [RUNSTACK_ADD retval] [arg0] [arg1] ... JSR  →  stack at JSR: [retval][args]
        // The callee then writes the return value into the retval slot which lives below
        // all parameters on its stack frame.
        if is_user {
            if let Some(sig) = &user_sig {
                if sig.return_type != NwType::Void {
                    // C++ AddVariableToStack / AddStructureToStack recurses over
                    // each field and emits one RUNSTACK_ADD with that field's aux.
                    // Vector returns use FLOAT aux per slot; user struct returns
                    // walk the fields. Engine-struct returns use their own aux.
                    let ret_slots = self.type_size(sig.return_type, &sig.return_type_name) / 4;
                    let per_slot_aux = match sig.return_type {
                        NwType::Vector => NwType::Float.auxcode(),
                        _ => sig.return_type.auxcode(),
                    };
                    for _ in 0..ret_slots {
                        self.emit_op(Opcode::RunstackAdd, per_slot_aux);
                        self.stack_depth += 1;
                    }
                }
            }
        }

        // Collect the provided argument expressions (declaration order).
        let mut provided: Vec<NodeId> = Vec::new();
        let mut arg_node = aid.right;
        while arg_node != NULL_NODE {
            let arg = self.arena.get(arg_node).clone();
            if arg.left != NULL_NODE { provided.push(arg.left); }
            arg_node = arg.right;
        }

        // Per-parameter info (cloned out of the owned signature to avoid borrow conflicts).
        let params_info: Vec<(NwType, Option<String>, Option<crate::semcheck::DefaultValue>)> =
            sig_opt.as_ref()
                .map(|s| s.params.iter()
                    .map(|p| (p.nw_type, p.type_name.clone(), p.default_value.clone()))
                    .collect())
                .unwrap_or_default();

        // Total args actually passed = provided + trailing defaults.
        let declared = params_info.len();
        let mut total = provided.len();
        if !params_info.is_empty() {
            while total < declared {
                let has_default = sig_opt.as_ref().unwrap().params[total].has_default;
                if !has_default { break; }
                total += 1;
            }
        }

        // Emission order: ENGINE (EXECUTE_COMMAND) calls push args right-to-left so the
        // FIRST declared parameter ends on TOP of the stack — the NWScript engine ABI
        // (the command handler pops the first parameter first). User/unknown calls keep
        // forward order (Rust assigns user-function parameter offsets to match a forward
        // push, so user calls are self-consistent either way).
        let is_engine = engine_sig.is_some();
        let order: Vec<usize> = if is_engine {
            (0..total).rev().collect()
        } else {
            (0..total).collect()
        };

        // arg_types indexed by parameter position (order-independent sum below).
        let mut arg_types: Vec<NwType> = vec![NwType::Void; total];
        for &i in &order {
            let is_action = params_info.get(i).map(|p| p.0 == NwType::Action).unwrap_or(false);
            let t = if i < provided.len() {
                let node = provided[i];
                if is_action {
                    // STORE_STATE per scriptcompfinalcode.cpp:1637-1647 (aux 0x10,
                    // word1 = global size, word2 = local stack bytes) + JMP-over-body.
                    self.emit_op(Opcode::StoreState, 0x10);
                    self.emit_i32(self.global_var_size);
                    self.emit_i32(self.stack_depth * 4);
                    let jmp_over = self.emit_jmp_placeholder(Opcode::Jmp);
                    self.generate_expr(node)?;
                    self.emit_op(Opcode::Ret, 0);
                    self.patch_jmp_here(jmp_over);
                    NwType::Action
                } else {
                    self.generate_expr(node)?
                }
            } else {
                // Trailing default value for parameter i.
                let (pt, ptn, dv) = params_info[i].clone();
                match dv {
                    Some(crate::semcheck::DefaultValue::EngineStruct) | None => {
                        self.emit_default_constant(pt, &ptn);
                    }
                    Some(d) => self.emit_default_value_from(&d),
                }
                pt
            };
            arg_types[i] = t;
        }
        let arg_count = total as u8;

        // Action-typed arguments push no runtime value (STORE_STATE handles them),
        // so they contribute 0 to the SP cleanup C++ performs after the call.
        let arg_byte_size: i32 = (0..arg_types.len())
            .map(|i| {
                let t = arg_types[i];
                if t == NwType::Action { return 0; }
                let tn = params_info.get(i).and_then(|p| p.1.clone());
                self.type_size(t, &tn)
            })
            .sum::<i32>();

        // Engine function
        if let Some((action_id, sig)) = engine_sig {
            self.emit_op(Opcode::ExecuteCommand, 0);
            self.emit_u16(action_id);
            self.emit(arg_count);
            self.stack_depth -= arg_byte_size / 4;
            if sig.return_type != NwType::Void {
                let ret_slots = self.type_size(sig.return_type, &sig.return_type_name) / 4;
                self.stack_depth += ret_slots;
            }
            return Ok(sig.return_type);
        }

        // User-defined function: arguments and return slot were both already pushed.
        if let Some(sig) = user_sig {
            self.emit_jsr_label(&func_name);
            // Pop the arguments — the return value remains at the top of the stack.
            if arg_byte_size > 0 {
                self.emit_modify_sp(-arg_byte_size);
                self.stack_depth -= arg_byte_size / 4;
            }
            return Ok(sig.return_type);
        }

        // Unknown function — emit placeholder
        self.emit_op(Opcode::ExecuteCommand, 0);
        self.emit_u16(0);
        self.emit(arg_count);
        self.stack_depth -= arg_byte_size / 4;
        Ok(NwType::Void)
    }

    fn emit_default_constant(&mut self, nw_type: NwType, _type_name: &Option<String>) {
        match nw_type {
            NwType::Integer => self.emit_const_int(0),
            NwType::Float => self.emit_const_float(0.0),
            NwType::String => self.emit_const_string(""),
            NwType::Object => self.emit_const_object(1), // OBJECT_INVALID
            NwType::Vector => {
                self.emit_const_float(0.0);
                self.emit_const_float(0.0);
                self.emit_const_float(0.0);
            }
            // C++ scriptcompfinalcode.cpp:1936-2010 emits the engine-struct-specific
            // CONSTANT for omitted defaults: aux = 0x10+n; json has u16-prefixed payload.
            NwType::EngineStructure(2) => {
                // location: CONSTANT aux=0x12 + 4-byte i32 0
                self.emit_op(Opcode::Constant, 0x12);
                self.emit_i32(0);
                self.stack_depth += 1;
            }
            NwType::EngineStructure(7) => {
                // json: CONSTANT aux=0x17 + u16 length + bytes
                self.emit_op(Opcode::Constant, 0x17);
                self.emit_str("");
                self.stack_depth += 1;
            }
            NwType::EngineStructure(n) => {
                // Other engine structs don't legally have defaults (validated in
                // semcheck), but stay consistent if reached.
                self.emit_op(Opcode::Constant, 0x10 + n);
                self.emit_i32(0);
                self.stack_depth += 1;
            }
            _ => self.emit_const_int(0),
        }
    }

    fn emit_default_value_from(&mut self, dv: &crate::semcheck::DefaultValue) {
        use crate::semcheck::DefaultValue as DV;
        match dv {
            DV::Integer(v) => self.emit_const_int(*v),
            DV::Float(v) => self.emit_const_float(*v),
            DV::String(s) => self.emit_const_string(s),
            DV::Object(v) => self.emit_const_object(*v),
            DV::Vector(x, y, z) => {
                self.emit_const_float(*x);
                self.emit_const_float(*y);
                self.emit_const_float(*z);
            }
            // C++ emits the engine-struct CONSTANT with its specific aux code.
            // We don't know which engine struct here because the DefaultValue enum
            // doesn't carry the index; emit a generic ENGST0 placeholder.
            DV::EngineStruct => {
                self.emit_op(Opcode::Constant, 0x10);
                self.emit_i32(0);
                self.stack_depth += 1;
            }
        }
    }

    fn gen_struct_field_read(&mut self, node_id: NodeId) -> Result<NwType, CompileError> {
        let (_t, _n) = self.gen_struct_field_read_typed(node_id)?;
        Ok(_t)
    }

    /// Returns (resulting type, struct-name if the result is a Struct).
    /// Carrying the struct name avoids relying on `type_name` being populated on
    /// chained `StructurePart` nodes (the parser only sets it on `Variable`).
    fn gen_struct_field_read_typed(
        &mut self,
        node_id: NodeId,
    ) -> Result<(NwType, Option<String>), CompileError> {
        // This helper self-recurses one frame per `.field` in a chain, bypassing the
        // generate_expr depth guard. Count it on the same expr_depth so a pathological
        // `v.a.a.a...(thousands).x` chain returns cleanly instead of overflowing.
        self.expr_depth += 1;
        if self.expr_depth > 2000 {
            self.expr_depth -= 1;
            return Err(CompileError::UnexpectedCharacter);
        }
        let r = self.gen_struct_field_read_typed_inner(node_id);
        self.expr_depth -= 1;
        r
    }

    fn gen_struct_field_read_typed_inner(
        &mut self,
        node_id: NodeId,
    ) -> Result<(NwType, Option<String>), CompileError> {
        let node = self.arena.get(node_id).clone();
        let field_name = node.string_data.as_deref().unwrap_or("").to_string();

        // Generate the struct value onto the stack and discover its struct name.
        let (struct_type, struct_name) = if node.left != NULL_NODE {
            let left = self.arena.get(node.left).clone();
            if left.op == Operation::StructurePart {
                let (t, n) = self.gen_struct_field_read_typed(node.left)?;
                (t, n)
            } else {
                let t = self.generate_expr(node.left)?;
                // The parser doesn't populate type_name on Variable / call nodes;
                // resolve via the runtime symbol tables.
                let n = match left.op {
                    Operation::Variable => {
                        let name = left.string_data.as_deref().unwrap_or("");
                        self.find_var(name).and_then(|(_, _, _, tn, _)| tn)
                    }
                    Operation::Action => {
                        let aid = if left.left != NULL_NODE {
                            self.arena.get(left.left).clone()
                        } else { return Ok((NwType::Void, None)); };
                        let fname = aid.string_data.as_deref().unwrap_or("");
                        self.func_sigs.iter()
                            .find(|f| f.name == fname)
                            .and_then(|f| f.return_type_name.clone())
                    }
                    _ => left.type_name.clone(),
                };
                (t, n)
            }
        } else {
            (NwType::Void, None)
        };

        if struct_type == NwType::Vector {
            let field_offset = match field_name.as_str() {
                "x" => 0i32,
                "y" => 4,
                "z" => 8,
                _ => return Ok((NwType::Void, None)),
            };
            self.emit_op(Opcode::DeStruct, 0x01);
            self.emit_u16(12);
            self.emit_u16(field_offset as u16);
            self.emit_u16(4);
            self.stack_depth -= 2;
            return Ok((NwType::Float, None));
        }

        if struct_type == NwType::Struct {
            let type_name = struct_name.as_deref().unwrap_or("");
            if let Some((field_off, field_sz, field_type)) = self.struct_field_offset(type_name, &field_name) {
                let struct_sz = self.struct_size(type_name);
                self.emit_op(Opcode::DeStruct, 0x01);
                self.emit_u16(struct_sz as u16);
                self.emit_u16(field_off as u16);
                self.emit_u16(field_sz as u16);
                self.stack_depth -= (struct_sz / 4) - (field_sz / 4);
                // If the field is itself a struct, look up its type_name from the
                // current struct definition so chained reads can continue.
                let inner_name = if field_type == NwType::Struct {
                    self.struct_defs.iter()
                        .find(|s| s.name == type_name)
                        .and_then(|s| s.fields.iter().find(|f| f.name == field_name))
                        .and_then(|f| f.type_name.clone())
                } else {
                    None
                };
                return Ok((field_type, inner_name));
            }
        }

        Ok((NwType::Void, None))
    }

    fn gen_struct_field_assign(&mut self, lhs: &crate::ast::AstNode) -> Result<(), CompileError> {
        // lhs is a StructurePart node: lhs.left = struct expression, lhs.string_data = field name.
        // The new value is already pushed on the stack by the caller.
        // Walk chained .field references down to the root Variable, summing offsets.
        let field_name = lhs.string_data.as_deref().unwrap_or("");
        if lhs.left == NULL_NODE { return Ok(()); }

        // Build the field path from outer to inner: [(field_name)] for each StructurePart,
        // then the root Variable.
        let mut path: Vec<String> = vec![field_name.to_string()];
        let mut node_id = lhs.left;
        loop {
            let n = self.arena.get(node_id).clone();
            match n.op {
                Operation::StructurePart => {
                    path.push(n.string_data.as_deref().unwrap_or("").to_string());
                    if n.left == NULL_NODE { return Ok(()); }
                    node_id = n.left;
                }
                Operation::Variable => break,
                _ => return Ok(()), // unsupported lvalue
            }
        }

        let root = self.arena.get(node_id).clone();
        let var_name = root.string_data.as_deref().unwrap_or("");
        let (so, _struct_sz, _, type_name, is_global) = match self.find_var(var_name) {
            Some(v) => v,
            None => return Ok(()),
        };

        // Resolve each nested field, accumulating offset and tracking current container type
        let mut cur_type_name = type_name.clone();
        let mut cur_offset = 0i32;
        let mut cur_size = 4i32;
        // path is outer-to-inner; we need inner-to-outer to walk from root → leaf.
        for field in path.iter().rev() {
            if let Some(tn) = &cur_type_name {
                if let Some((off, sz, ftype)) = self.struct_field_offset(tn, field) {
                    cur_offset += off;
                    cur_size = sz;
                    cur_type_name = if ftype == NwType::Struct {
                        // Look up the field's struct name via the struct definition
                        self.struct_defs.iter()
                            .find(|s| s.name == *tn)
                            .and_then(|s| s.fields.iter().find(|f| f.name == *field))
                            .and_then(|f| f.type_name.clone())
                    } else {
                        None
                    };
                } else {
                    return Ok(());
                }
            } else {
                // Vector (no type_name): only valid as innermost
                let (off, sz) = match field.as_str() {
                    "x" => (0i32, 4i32),
                    "y" => (4, 4),
                    "z" => (8, 4),
                    _ => return Ok(()),
                };
                cur_offset += off;
                cur_size = sz;
                cur_type_name = None;
            }
        }

        if is_global {
            let bp_offset = so + cur_offset - self.global_var_size;
            self.emit_op(Opcode::AssignmentBase, 0x01);
            self.emit_i32(bp_offset);
            self.emit_u16(cur_size as u16);
        } else {
            let target_offset = so + cur_offset - self.stack_depth * 4;
            self.emit_op(Opcode::Assignment, 0x01);
            self.emit_i32(target_offset);
            self.emit_u16(cur_size as u16);
        }
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
    fn test_logical_and_opcode() {
        // C++ NWScript emits LogicalAnd (0x06) for && — the result is normalised to 0/1
        // rather than short-circuited via JZ.
        let ncs = compile_to_ncs("void main() { int x = 1 && 0; }");
        assert!(ncs.contains(&(Opcode::LogicalAnd as u8)));
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
