use crate::ast::{AstArena, AstNode, NodeId, NULL_NODE, Operation};
use crate::errors::{CompileError, Diagnostic};
use crate::opcode::Opcode;
use crate::types::NwType;

const NCS_HEADER_BYTES: &[u8] = b"NCS V1.0";

#[derive(Debug, Clone)]
struct FuncInfo {
    name: String,
    return_type: NwType,
    code_start: usize,
    code_end: usize,
    param_count: usize,
    param_types: Vec<NwType>,
    param_sizes: Vec<i32>,
}

#[derive(Debug, Clone)]
struct LocalVar {
    name: String,
    nw_type: NwType,
    stack_offset: i32,
    scope_level: u32,
    size: i32,
}

pub struct CodeGenerator<'a> {
    arena: &'a AstArena,
    file_names: &'a [String],
    code: Vec<u8>,
    pub diagnostics: Vec<Diagnostic>,
    functions: Vec<FuncInfo>,
    locals: Vec<LocalVar>,
    scope_level: u32,
    stack_depth: i32,
    collect_all_errors: bool,
    current_func_idx: Option<usize>,
    loop_start_stack: Vec<usize>,
    loop_exit_fixups: Vec<Vec<usize>>,
    switch_exit_fixups: Vec<Vec<usize>>,
}

impl<'a> CodeGenerator<'a> {
    pub fn new(arena: &'a AstArena, file_names: &'a [String]) -> Self {
        Self {
            arena,
            file_names,
            code: Vec::with_capacity(8192),
            diagnostics: Vec::new(),
            functions: Vec::new(),
            locals: Vec::new(),
            scope_level: 0,
            stack_depth: 0,
            collect_all_errors: false,
            current_func_idx: None,
            loop_start_stack: Vec::new(),
            loop_exit_fixups: Vec::new(),
            switch_exit_fixups: Vec::new(),
        }
    }

    pub fn set_collect_all_errors(&mut self, v: bool) {
        self.collect_all_errors = v;
    }

    fn emit(&mut self, byte: u8) {
        self.code.push(byte);
    }

    fn emit_op(&mut self, opcode: Opcode, auxcode: u8) {
        self.emit(opcode as u8);
        self.emit(auxcode);
    }

    fn emit_i32(&mut self, v: i32) {
        self.code.extend_from_slice(&v.to_be_bytes());
    }

    fn emit_u16(&mut self, v: u16) {
        self.code.extend_from_slice(&v.to_be_bytes());
    }

    fn emit_f32(&mut self, v: f32) {
        self.code.extend_from_slice(&v.to_be_bytes());
    }

    fn emit_string(&mut self, s: &str) {
        self.emit_u16(s.len() as u16);
        self.code.extend_from_slice(s.as_bytes());
    }

    fn current_offset(&self) -> usize {
        self.code.len()
    }

    fn patch_i32(&mut self, offset: usize, value: i32) {
        let bytes = value.to_be_bytes();
        self.code[offset] = bytes[0];
        self.code[offset + 1] = bytes[1];
        self.code[offset + 2] = bytes[2];
        self.code[offset + 3] = bytes[3];
    }

    fn emit_jmp(&mut self, opcode: Opcode) -> usize {
        self.emit_op(opcode, 0);
        let fixup = self.current_offset();
        self.emit_i32(0);
        fixup
    }

    fn patch_jmp(&mut self, fixup: usize) {
        let target = self.current_offset() as i32;
        let origin = fixup as i32 - 2;
        self.patch_i32(fixup, target - origin);
    }

    fn emit_jmp_to(&mut self, opcode: Opcode, target: usize) {
        self.emit_op(opcode, 0);
        let origin = self.current_offset() as i32 - 2;
        self.emit_i32(target as i32 - origin);
    }

    fn emit_modify_sp(&mut self, amount: i32) {
        if amount != 0 {
            self.emit_op(Opcode::ModifyStackPointer, 0);
            self.emit_i32(amount);
            self.stack_depth += amount / 4;
        }
    }

    fn emit_const_int(&mut self, value: i32) {
        self.emit_op(Opcode::Constant, NwType::Integer.auxcode());
        self.emit_i32(value);
        self.stack_depth += 1;
    }

    fn emit_const_float(&mut self, value: f32) {
        self.emit_op(Opcode::Constant, NwType::Float.auxcode());
        self.emit_f32(value);
        self.stack_depth += 1;
    }

    fn emit_const_string(&mut self, s: &str) {
        self.emit_op(Opcode::Constant, NwType::String.auxcode());
        self.emit_string(s);
        self.stack_depth += 1;
    }

    fn emit_const_object(&mut self, value: i32) {
        self.emit_op(Opcode::Constant, NwType::Object.auxcode());
        self.emit_i32(value);
        self.stack_depth += 1;
    }

    fn find_local(&self, name: &str) -> Option<(i32, i32, NwType)> {
        self.locals.iter().rev().find(|l| l.name == name).map(|l| {
            (l.stack_offset, l.size, l.nw_type)
        })
    }

    pub fn generate(&mut self, root: NodeId) -> Result<Vec<u8>, CompileError> {
        self.code.clear();

        // NCS header
        self.code.extend_from_slice(NCS_HEADER_BYTES);
        self.emit(b'B');
        let size_offset = self.current_offset();
        self.emit_i32(0);

        // Collect function info
        self.collect_functions(root);

        // Generate loader
        self.generate_loader()?;

        // Generate functions
        self.generate_functions(root)?;

        // Patch total size
        let total = self.code.len() as i32;
        self.patch_i32(size_offset, total);

        Ok(self.code.clone())
    }

    fn collect_functions(&mut self, node_id: NodeId) {
        if node_id == NULL_NODE { return; }
        let node = self.arena.get(node_id).clone();
        if node.op == Operation::FunctionalUnit {
            self.collect_functions(node.left);
            self.collect_functions(node.right);
            return;
        }
        if node.op == Operation::Function && node.left != NULL_NODE {
            let fid = self.arena.get(node.left).clone();
            let name = fid.string_data.as_deref().unwrap_or("").to_string();
            let mut param_types = Vec::new();
            let mut param_sizes = Vec::new();
            let mut pnode = fid.left;
            while pnode != NULL_NODE {
                let p = self.arena.get(pnode).clone();
                if p.op == Operation::FunctionParamName {
                    param_types.push(p.nw_type);
                    param_sizes.push(p.nw_type.size_bytes());
                }
                pnode = p.right;
            }
            self.functions.push(FuncInfo {
                name,
                return_type: fid.nw_type,
                code_start: 0,
                code_end: 0,
                param_count: param_types.len(),
                param_types,
                param_sizes,
            });
        }
    }

    fn find_func_idx(&self, name: &str) -> Option<usize> {
        self.functions.iter().position(|f| f.name == name)
    }

    fn generate_loader(&mut self) -> Result<(), CompileError> {
        // JSR to main (placeholder, will be patched)
        let _jsr_fixup = self.emit_jmp(Opcode::Jsr);
        self.emit_op(Opcode::Ret, 0);
        Ok(())
    }

    fn generate_functions(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        if node_id == NULL_NODE { return Ok(()); }
        let node = self.arena.get(node_id).clone();
        if node.op == Operation::FunctionalUnit {
            self.generate_functions(node.left)?;
            self.generate_functions(node.right)?;
            return Ok(());
        }
        if node.op == Operation::Function {
            self.generate_function(node_id)?;
        }
        Ok(())
    }

    fn generate_function(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        if node.left == NULL_NODE { return Ok(()); }
        let fid = self.arena.get(node.left).clone();
        let func_name = fid.string_data.as_deref().unwrap_or("").to_string();

        let func_idx = self.find_func_idx(&func_name);
        if let Some(idx) = func_idx {
            self.functions[idx].code_start = self.current_offset();
            self.current_func_idx = Some(idx);
        }

        let saved_stack = self.stack_depth;
        let saved_locals = self.locals.len();
        self.scope_level += 1;

        // Register params as locals
        let mut param_node = fid.left;
        while param_node != NULL_NODE {
            let p = self.arena.get(param_node).clone();
            if p.op == Operation::FunctionParamName {
                let name = p.string_data.as_deref().unwrap_or("").to_string();
                let size = p.nw_type.size_bytes();
                self.locals.push(LocalVar {
                    name,
                    nw_type: p.nw_type,
                    stack_offset: 0,
                    scope_level: self.scope_level,
                    size,
                });
            }
            param_node = p.right;
        }

        // Generate body
        if node.right != NULL_NODE {
            self.generate_statement(node.right)?;
        }

        // Epilogue
        self.emit_op(Opcode::Ret, 0);

        self.locals.truncate(saved_locals);
        self.scope_level -= 1;
        self.stack_depth = saved_stack;

        if let Some(idx) = func_idx {
            self.functions[idx].code_end = self.current_offset();
        }
        self.current_func_idx = None;
        Ok(())
    }

    fn generate_statement(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        if node_id == NULL_NODE { return Ok(()); }
        let node = self.arena.get(node_id).clone();

        match node.op {
            Operation::CompoundStatement => {
                self.scope_level += 1;
                let saved_locals = self.locals.len();
                let saved_depth = self.stack_depth;
                self.generate_statement(node.left)?;
                let alloc: i32 = self.locals[saved_locals..].iter().map(|l| l.size).sum();
                if alloc > 0 { self.emit_modify_sp(-alloc); }
                self.locals.truncate(saved_locals);
                self.scope_level -= 1;
                self.stack_depth = saved_depth;
            }
            Operation::StatementList => {
                self.generate_statement(node.left)?;
                self.generate_statement(node.right)?;
            }
            Operation::Statement | Operation::StatementNoDebug => {
                self.generate_statement(node.left)?;
            }
            Operation::KeywordDeclaration | Operation::ConstDeclaration => {
                self.generate_local_decl(node_id)?;
            }
            Operation::IfBlock => { self.generate_if(node_id)?; }
            Operation::WhileBlock => { self.generate_while(node_id)?; }
            Operation::DoWhileBlock => { self.generate_do_while(node_id)?; }
            Operation::ForBlock => { self.generate_statement(node.left)?; }
            Operation::SwitchBlock => { self.generate_switch(node_id)?; }
            Operation::Return => { self.generate_return(node_id)?; }
            Operation::Break => {
                let fixup = self.emit_jmp(Opcode::Jmp);
                if let Some(exits) = self.loop_exit_fixups.last_mut().or(self.switch_exit_fixups.last_mut()) {
                    exits.push(fixup);
                }
            }
            Operation::Continue => {
                if let Some(&target) = self.loop_start_stack.last() {
                    self.emit_jmp_to(Opcode::Jmp, target);
                }
            }
            _ => { self.generate_expr(node_id)?; }
        }
        Ok(())
    }

    fn generate_local_decl(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        if node.left == NULL_NODE { return Ok(()); }
        let type_node = self.arena.get(node.left).clone();
        let nw_type = op_to_type(type_node.op);
        let size = nw_type.size_bytes();

        let mut vl_id = type_node.left;
        while vl_id != NULL_NODE {
            let vl = self.arena.get(vl_id).clone();
            if vl.left != NULL_NODE {
                let var = self.arena.get(vl.left).clone();
                let name = var.string_data.as_deref().unwrap_or("").to_string();
                if var.left != NULL_NODE {
                    self.generate_expr(var.left)?;
                } else {
                    match nw_type {
                        NwType::Integer => self.emit_const_int(0),
                        NwType::Float => self.emit_const_float(0.0),
                        NwType::String => self.emit_const_string(""),
                        NwType::Object => self.emit_const_object(0x7f000000u32 as i32),
                        _ => self.emit_const_int(0),
                    }
                }
                self.locals.push(LocalVar {
                    name, nw_type,
                    stack_offset: -(self.stack_depth * 4) as i32,
                    scope_level: self.scope_level, size,
                });
            }
            vl_id = vl.right;
        }
        Ok(())
    }

    fn generate_if(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        if node.left != NULL_NODE {
            let cond = self.arena.get(node.left).clone();
            if cond.left != NULL_NODE { self.generate_expr(cond.left)?; }
        }
        let jz_fixup = self.emit_jmp(Opcode::Jz);
        self.stack_depth -= 1;
        if node.right != NULL_NODE {
            let choice = self.arena.get(node.right).clone();
            self.generate_statement(choice.left)?;
            if choice.right != NULL_NODE {
                let jmp_fixup = self.emit_jmp(Opcode::Jmp);
                self.patch_jmp(jz_fixup);
                self.generate_statement(choice.right)?;
                self.patch_jmp(jmp_fixup);
            } else {
                self.patch_jmp(jz_fixup);
            }
        } else {
            self.patch_jmp(jz_fixup);
        }
        Ok(())
    }

    fn generate_while(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        let loop_start = self.current_offset();
        self.loop_start_stack.push(loop_start);
        self.loop_exit_fixups.push(Vec::new());
        if node.left != NULL_NODE {
            let cond = self.arena.get(node.left).clone();
            if cond.left != NULL_NODE { self.generate_expr(cond.left)?; }
        }
        let jz_fixup = self.emit_jmp(Opcode::Jz);
        self.stack_depth -= 1;
        if node.right != NULL_NODE {
            let choice = self.arena.get(node.right).clone();
            self.generate_statement(choice.left)?;
        }
        self.emit_jmp_to(Opcode::Jmp, loop_start);
        self.patch_jmp(jz_fixup);
        let exits = self.loop_exit_fixups.pop().unwrap_or_default();
        for f in exits { self.patch_jmp(f); }
        self.loop_start_stack.pop();
        Ok(())
    }

    fn generate_do_while(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        let loop_start = self.current_offset();
        self.loop_start_stack.push(loop_start);
        self.loop_exit_fixups.push(Vec::new());
        self.generate_statement(node.left)?;
        if node.right != NULL_NODE {
            let cond = self.arena.get(node.right).clone();
            if cond.left != NULL_NODE { self.generate_expr(cond.left)?; }
        }
        self.emit_jmp_to(Opcode::Jnz, loop_start);
        self.stack_depth -= 1;
        let exits = self.loop_exit_fixups.pop().unwrap_or_default();
        for f in exits { self.patch_jmp(f); }
        self.loop_start_stack.pop();
        Ok(())
    }

    fn generate_switch(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        self.switch_exit_fixups.push(Vec::new());
        if node.left != NULL_NODE {
            let cond = self.arena.get(node.left).clone();
            if cond.left != NULL_NODE { self.generate_expr(cond.left)?; }
        }
        self.generate_switch_body(node.right)?;
        self.emit_modify_sp(-4);
        let exits = self.switch_exit_fixups.pop().unwrap_or_default();
        for f in exits { self.patch_jmp(f); }
        Ok(())
    }

    fn generate_switch_body(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        if node_id == NULL_NODE { return Ok(()); }
        let node = self.arena.get(node_id).clone();
        if node.op == Operation::StatementList {
            self.generate_switch_body(node.left)?;
            self.generate_switch_body(node.right)?;
            return Ok(());
        }
        match node.op {
            Operation::Case => {
                self.emit_op(Opcode::RunstackCopy, 0);
                self.emit_i32(-4);
                self.emit_u16(4);
                self.stack_depth += 1;
                if node.left != NULL_NODE { self.generate_expr(node.left)?; }
                self.emit_op(Opcode::Equal, 0x20);
                self.stack_depth -= 1;
                let jz = self.emit_jmp(Opcode::Jz);
                self.stack_depth -= 1;
                self.patch_jmp(jz);
            }
            Operation::Default => {}
            _ => { self.generate_statement(node_id)?; }
        }
        Ok(())
    }

    fn generate_return(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();
        if node.left != NULL_NODE { self.generate_expr(node.left)?; }
        self.emit_op(Opcode::Ret, 0);
        Ok(())
    }

    fn generate_expr(&mut self, node_id: NodeId) -> Result<NwType, CompileError> {
        if node_id == NULL_NODE { return Ok(NwType::Void); }
        let node = self.arena.get(node_id).clone();

        match node.op {
            Operation::ConstantInteger => {
                self.emit_const_int(node.int_data[0]);
                Ok(NwType::Integer)
            }
            Operation::ConstantFloat => {
                self.emit_const_float(node.float_data);
                Ok(NwType::Float)
            }
            Operation::ConstantString => {
                self.emit_const_string(node.string_data.as_deref().unwrap_or(""));
                Ok(NwType::String)
            }
            Operation::ConstantObject => {
                self.emit_const_object(node.int_data[0]);
                Ok(NwType::Object)
            }
            Operation::ConstantVector => {
                self.emit_const_float(node.vector_data[0]);
                self.emit_const_float(node.vector_data[1]);
                self.emit_const_float(node.vector_data[2]);
                Ok(NwType::Vector)
            }
            Operation::ConstantJson => {
                self.emit_op(Opcode::Constant, 0x17);
                self.emit_u16(0);
                self.stack_depth += 1;
                Ok(NwType::EngineStructure(7))
            }
            Operation::ConstantLocation => {
                self.emit_op(Opcode::Constant, 0x12);
                self.emit_i32(0);
                self.stack_depth += 1;
                Ok(NwType::EngineStructure(2))
            }
            Operation::Variable => {
                let name = node.string_data.as_deref().unwrap_or("");
                if let Some((so, sz, nt)) = self.find_local(name) {
                    let offset = so - (self.stack_depth * 4) as i32;
                    self.emit_op(Opcode::RunstackCopy, 0);
                    self.emit_i32(offset);
                    self.emit_u16(sz as u16);
                    self.stack_depth += sz / 4;
                    return Ok(nt);
                }
                self.emit_const_int(0);
                Ok(NwType::Integer)
            }
            Operation::Assignment => {
                let right_type = self.generate_expr(node.right)?;
                if node.left != NULL_NODE {
                    let lhs = self.arena.get(node.left).clone();
                    if lhs.op == Operation::Variable {
                        let name = lhs.string_data.as_deref().unwrap_or("");
                        if let Some((so, sz, _)) = self.find_local(name) {
                            let offset = so - (self.stack_depth * 4) as i32;
                            self.emit_op(Opcode::Assignment, 0);
                            self.emit_i32(offset);
                            self.emit_u16(sz as u16);
                        }
                    }
                }
                Ok(right_type)
            }
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
            Operation::LogicalAnd => {
                self.generate_expr(node.left)?;
                let jz = self.emit_jmp(Opcode::Jz);
                self.stack_depth -= 1;
                self.generate_expr(node.right)?;
                let jmp = self.emit_jmp(Opcode::Jmp);
                self.stack_depth -= 1;
                self.patch_jmp(jz);
                self.emit_const_int(0);
                self.patch_jmp(jmp);
                Ok(NwType::Integer)
            }
            Operation::LogicalOr => {
                self.generate_expr(node.left)?;
                let jnz = self.emit_jmp(Opcode::Jnz);
                self.stack_depth -= 1;
                self.generate_expr(node.right)?;
                let jmp = self.emit_jmp(Opcode::Jmp);
                self.stack_depth -= 1;
                self.patch_jmp(jnz);
                self.emit_const_int(1);
                self.patch_jmp(jmp);
                Ok(NwType::Integer)
            }
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
            Operation::PostIncrement | Operation::PreIncrement => {
                if node.left != NULL_NODE {
                    let lhs = self.arena.get(node.left).clone();
                    if lhs.op == Operation::Variable {
                        let vname = lhs.string_data.as_deref().unwrap_or("");
                        if let Some((so, _, _)) = self.find_local(vname) {
                            let offset = so - (self.stack_depth * 4) as i32;
                            self.emit_op(Opcode::Increment, 0x03);
                            self.emit_i32(offset);
                        }
                    }
                }
                Ok(NwType::Integer)
            }
            Operation::PostDecrement | Operation::PreDecrement => {
                if node.left != NULL_NODE {
                    let lhs = self.arena.get(node.left).clone();
                    if lhs.op == Operation::Variable {
                        let vname = lhs.string_data.as_deref().unwrap_or("");
                        if let Some((so, _, _)) = self.find_local(vname) {
                            let offset = so - (self.stack_depth * 4) as i32;
                            self.emit_op(Opcode::Decrement, 0x03);
                            self.emit_i32(offset);
                        }
                    }
                }
                Ok(NwType::Integer)
            }
            Operation::Action => {
                if node.left == NULL_NODE { return Ok(NwType::Void); }
                let aid = self.arena.get(node.left).clone();
                let func_name = aid.string_data.as_deref().unwrap_or("");
                let mut arg_count = 0;
                let mut arg_node = aid.right;
                while arg_node != NULL_NODE {
                    let arg = self.arena.get(arg_node).clone();
                    if arg.left != NULL_NODE {
                        self.generate_expr(arg.left)?;
                        arg_count += 1;
                    }
                    arg_node = arg.right;
                }
                if let Some(idx) = self.find_func_idx(func_name) {
                    let fi = self.functions[idx].clone();
                    let _jsr = self.emit_jmp(Opcode::Jsr);
                    let arg_size = arg_count * 4;
                    if arg_size > 0 { self.emit_modify_sp(-(arg_size as i32)); }
                    return Ok(fi.return_type);
                }
                self.emit_op(Opcode::ExecuteCommand, 0);
                self.emit_u16(0);
                self.emit(arg_count as u8);
                self.stack_depth -= arg_count;
                Ok(NwType::Void)
            }
            Operation::CondBlock => {
                if node.left != NULL_NODE {
                    let cond = self.arena.get(node.left).clone();
                    self.generate_expr(cond.left)?;
                }
                let jz = self.emit_jmp(Opcode::Jz);
                self.stack_depth -= 1;
                let mut rt = NwType::Void;
                if node.right != NULL_NODE {
                    let choice = self.arena.get(node.right).clone();
                    rt = self.generate_expr(choice.left)?;
                    let jmp = self.emit_jmp(Opcode::Jmp);
                    self.stack_depth -= 1;
                    self.patch_jmp(jz);
                    self.generate_expr(choice.right)?;
                    self.patch_jmp(jmp);
                } else {
                    self.patch_jmp(jz);
                }
                Ok(rt)
            }
            Operation::StructurePart => {
                self.generate_expr(node.left)?;
                Ok(NwType::Void)
            }
            Operation::ActionArgList => {
                self.generate_expr(node.left)
            }
            _ => Ok(NwType::Void),
        }
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

    fn compile_to_ncs(src: &str) -> Vec<u8> {
        let mut lexer = Lexer::new(src, "test.nss", 0);
        let tokens = lexer.tokenize();
        let mut parser = Parser::new(tokens);
        parser.file_names.push("test.nss".to_string());
        let root = parser.parse_program().unwrap();
        let mut cg = CodeGenerator::new(&parser.arena, &parser.file_names);
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
    fn test_deterministic_output() {
        let src = "void main() { int x = 1 + 2; }";
        let a = compile_to_ncs(src);
        let b = compile_to_ncs(src);
        assert_eq!(a, b);
    }

    #[test]
    fn test_constant_int() {
        let ncs = compile_to_ncs("void main() { int x = 42; }");
        assert!(ncs.windows(2).any(|w| w[0] == 0x04 && w[1] == 0x03));
    }

    #[test]
    fn test_constant_string() {
        let ncs = compile_to_ncs(r#"void main() { string s = "hello"; }"#);
        assert!(ncs.windows(5).any(|w| w == b"hello"));
    }

    #[test]
    fn test_add_instruction() {
        let ncs = compile_to_ncs("void main() { int x = 1 + 2; }");
        assert!(ncs.contains(&(Opcode::Add as u8)));
    }

    #[test]
    fn test_if_generates_jz() {
        let ncs = compile_to_ncs("void main() { if (1) { int x = 1; } }");
        assert!(ncs.contains(&(Opcode::Jz as u8)));
    }

    #[test]
    fn test_while_generates_jmp() {
        let ncs = compile_to_ncs("void main() { while (0) { } }");
        assert!(ncs.contains(&(Opcode::Jz as u8)));
        assert!(ncs.contains(&(Opcode::Jmp as u8)));
    }

    #[test]
    fn test_ret_instruction() {
        let ncs = compile_to_ncs("void main() { }");
        assert!(ncs.contains(&(Opcode::Ret as u8)));
    }

    #[test]
    fn test_multiple_functions() {
        let ncs = compile_to_ncs("int helper() { return 42; }\nvoid main() { }");
        let ret_count = ncs.iter().filter(|&&b| b == Opcode::Ret as u8).count();
        assert!(ret_count >= 2);
    }

    #[test]
    fn test_comparison() {
        let ncs = compile_to_ncs("void main() { int x = (1 > 2) ? 1 : 0; }");
        assert!(ncs.contains(&(Opcode::GT as u8)));
    }

    #[test]
    fn test_boolean_not() {
        let ncs = compile_to_ncs("void main() { int x = !0; }");
        assert!(ncs.contains(&(Opcode::BooleanNot as u8)));
    }

    #[test]
    fn test_ncs_size_in_header() {
        let ncs = compile_to_ncs("void main() { }");
        let size = i32::from_be_bytes([ncs[9], ncs[10], ncs[11], ncs[12]]);
        assert_eq!(size as usize, ncs.len());
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
    fn test_negation() {
        let ncs = compile_to_ncs("void main() { int x = -42; }");
        assert!(ncs.contains(&(Opcode::Negation as u8)));
    }
}
