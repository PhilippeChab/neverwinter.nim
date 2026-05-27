use crate::ast::{AstArena, AstNode, NodeId, NULL_NODE, Operation};
use crate::errors::{CompileError, Diagnostic};
use crate::opcode::{NCS_HEADER, Opcode};
use crate::types::NwType;

struct SymbolEntry {
    name: String,
    nw_type: NwType,
    type_name: Option<String>,
    is_function: bool,
    param_count: usize,
    param_types: Vec<NwType>,
    has_implementation: bool,
    code_start: usize,
    code_end: usize,
    return_type: NwType,
    scope_level: u32,
    stack_offset: i32,
    is_constant: bool,
    const_int: i32,
    const_float: f32,
    const_string: Option<String>,
}

struct StructDef {
    name: String,
    fields: Vec<StructField>,
    byte_size: i32,
}

struct StructField {
    name: String,
    nw_type: NwType,
    type_name: Option<String>,
    offset: i32,
}

pub struct CodeGenerator<'a> {
    arena: &'a AstArena,
    file_names: &'a [String],
    output: Vec<u8>,
    pub diagnostics: Vec<Diagnostic>,
    symbols: Vec<SymbolEntry>,
    structs: Vec<StructDef>,
    scope_level: u32,
    stack_depth: i32,
    collect_all_errors: bool,
    current_function: Option<String>,
    current_return_type: NwType,
    loop_depth: u32,
    switch_depth: u32,
    label_counter: u32,
    labels: Vec<(u32, usize)>,
    fixups: Vec<(u32, usize)>,
}

impl<'a> CodeGenerator<'a> {
    pub fn new(arena: &'a AstArena, file_names: &'a [String]) -> Self {
        Self {
            arena,
            file_names,
            output: Vec::with_capacity(4096),
            diagnostics: Vec::new(),
            symbols: Vec::new(),
            structs: Vec::new(),
            scope_level: 0,
            stack_depth: 0,
            collect_all_errors: false,
            current_function: None,
            current_return_type: NwType::Void,
            loop_depth: 0,
            switch_depth: 0,
            label_counter: 0,
            labels: Vec::new(),
            fixups: Vec::new(),
        }
    }

    pub fn set_collect_all_errors(&mut self, v: bool) {
        self.collect_all_errors = v;
    }

    fn error(&mut self, err: CompileError, node: &AstNode) -> Result<(), CompileError> {
        let file = if (node.file_id as usize) < self.file_names.len() {
            self.file_names[node.file_id as usize].clone()
        } else {
            String::from("<unknown>")
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

    fn new_label(&mut self) -> u32 {
        let l = self.label_counter;
        self.label_counter += 1;
        l
    }

    fn mark_label(&mut self, label: u32) {
        self.labels.push((label, self.output.len()));
    }

    fn emit_byte(&mut self, b: u8) {
        self.output.push(b);
    }

    fn emit_u16_be(&mut self, v: u16) {
        self.output.extend_from_slice(&v.to_be_bytes());
    }

    fn emit_i32_be(&mut self, v: i32) {
        self.output.extend_from_slice(&v.to_be_bytes());
    }

    fn emit_f32_be(&mut self, v: f32) {
        self.output.extend_from_slice(&v.to_be_bytes());
    }

    fn emit_string(&mut self, s: &str) {
        self.emit_u16_be(s.len() as u16);
        self.output.extend_from_slice(s.as_bytes());
    }

    fn emit_instruction(&mut self, opcode: Opcode, auxcode: u8) {
        self.emit_byte(opcode as u8);
        self.emit_byte(auxcode);
    }

    pub fn generate(&mut self, root: NodeId) -> Result<Vec<u8>, CompileError> {
        self.output.clear();

        // Write NCS header
        self.output.extend_from_slice(NCS_HEADER);
        // Placeholder for total size (4 bytes, will be patched)
        let size_offset = self.output.len();
        self.emit_byte(0);
        self.emit_i32_be(0);

        // First pass: collect declarations (structs, function prototypes, globals)
        self.collect_declarations(root)?;

        // Second pass: generate code
        self.generate_node(root)?;

        // Emit RET at end
        self.emit_instruction(Opcode::Ret, 0);

        // Patch the total size
        let total_size = self.output.len() as i32;
        let size_bytes = total_size.to_be_bytes();
        self.output[size_offset + 1] = size_bytes[0];
        self.output[size_offset + 2] = size_bytes[1];
        self.output[size_offset + 3] = size_bytes[2];
        self.output[size_offset + 4] = size_bytes[3];

        // Resolve label fixups
        self.resolve_fixups();

        Ok(self.output.clone())
    }

    fn collect_declarations(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        if node_id == NULL_NODE {
            return Ok(());
        }

        let node = self.arena.get(node_id).clone();

        match node.op {
            Operation::FunctionalUnit => {
                if node.left != NULL_NODE {
                    self.collect_declarations(node.left)?;
                }
                if node.right != NULL_NODE {
                    self.collect_declarations(node.right)?;
                }
            }
            Operation::KeywordStruct => {
                if let Some(def_id) = self.arena.try_get(node.left) {
                    let def = def_id.clone();
                    if def.op == Operation::StructureDefinition {
                        self.register_struct(def_id.clone())?;
                    }
                }
            }
            Operation::FunctionDeclaration | Operation::Function => {
                self.register_function(&node)?;
            }
            Operation::GlobalVariables => {
                // Will handle in code generation
            }
            Operation::ConstDeclaration => {
                self.register_constant(&node)?;
            }
            _ => {}
        }

        Ok(())
    }

    fn register_struct(&mut self, _node: AstNode) -> Result<(), CompileError> {
        // TODO: implement struct registration
        Ok(())
    }

    fn register_function(&mut self, _node: &AstNode) -> Result<(), CompileError> {
        // TODO: implement function registration
        Ok(())
    }

    fn register_constant(&mut self, _node: &AstNode) -> Result<(), CompileError> {
        // TODO: implement constant registration
        Ok(())
    }

    fn generate_node(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        if node_id == NULL_NODE {
            return Ok(());
        }

        let node = self.arena.get(node_id).clone();

        match node.op {
            Operation::FunctionalUnit => {
                self.generate_node(node.left)?;
                self.generate_node(node.right)?;
            }
            Operation::Function => {
                self.generate_function(node_id)?;
            }
            Operation::CompoundStatement => {
                self.scope_level += 1;
                self.generate_node(node.left)?;
                self.scope_level -= 1;
            }
            Operation::StatementList => {
                self.generate_node(node.left)?;
                self.generate_node(node.right)?;
            }
            Operation::Statement | Operation::StatementNoDebug => {
                self.generate_node(node.left)?;
            }
            Operation::Return => {
                self.generate_return(node_id)?;
            }
            Operation::KeywordDeclaration | Operation::ConstDeclaration => {
                // Local variable declaration - handled in semantic pass
                self.generate_node(node.left)?;
            }
            _ => {
                // For now, walk children
                if node.left != NULL_NODE {
                    self.generate_node(node.left)?;
                }
                if node.right != NULL_NODE {
                    self.generate_node(node.right)?;
                }
            }
        }

        Ok(())
    }

    fn generate_function(&mut self, _node_id: NodeId) -> Result<(), CompileError> {
        // TODO: full function code generation
        Ok(())
    }

    fn generate_return(&mut self, node_id: NodeId) -> Result<(), CompileError> {
        let node = self.arena.get(node_id).clone();

        if node.left != NULL_NODE {
            self.generate_expression(node.left)?;
        }

        self.emit_instruction(Opcode::Ret, 0);
        Ok(())
    }

    fn generate_expression(&mut self, node_id: NodeId) -> Result<NwType, CompileError> {
        if node_id == NULL_NODE {
            return Ok(NwType::Void);
        }

        let node = self.arena.get(node_id).clone();

        match node.op {
            Operation::ConstantInteger => {
                self.emit_instruction(Opcode::Constant, NwType::Integer.auxcode());
                self.emit_i32_be(node.int_data[0]);
                self.stack_depth += 4;
                Ok(NwType::Integer)
            }
            Operation::ConstantFloat => {
                self.emit_instruction(Opcode::Constant, NwType::Float.auxcode());
                self.emit_f32_be(node.float_data);
                self.stack_depth += 4;
                Ok(NwType::Float)
            }
            Operation::ConstantString => {
                let s = node.string_data.as_deref().unwrap_or("");
                self.emit_instruction(Opcode::Constant, NwType::String.auxcode());
                self.emit_string(s);
                self.stack_depth += 4;
                Ok(NwType::String)
            }
            Operation::ConstantObject => {
                self.emit_instruction(Opcode::Constant, NwType::Object.auxcode());
                self.emit_i32_be(node.int_data[0]);
                self.stack_depth += 4;
                Ok(NwType::Object)
            }
            Operation::Add | Operation::Subtract | Operation::Multiply
            | Operation::Divide | Operation::Modulus => {
                let left_type = self.generate_expression(node.left)?;
                let right_type = self.generate_expression(node.right)?;

                let opcode = match node.op {
                    Operation::Add => Opcode::Add,
                    Operation::Subtract => Opcode::Sub,
                    Operation::Multiply => Opcode::Mul,
                    Operation::Divide => Opcode::Div,
                    Operation::Modulus => Opcode::Modulus,
                    _ => unreachable!(),
                };

                if let Some(auxcode) = left_type.auxcode_pair(right_type) {
                    self.emit_instruction(opcode, auxcode);
                    self.stack_depth -= right_type.size_bytes();
                    Ok(left_type)
                } else {
                    self.error(CompileError::ArithmeticOperationHasInvalidOperands, &node)?;
                    Ok(NwType::Integer)
                }
            }
            Operation::Negation => {
                let t = self.generate_expression(node.left)?;
                self.emit_instruction(Opcode::Negation, t.auxcode());
                Ok(t)
            }
            Operation::BooleanNot => {
                self.generate_expression(node.left)?;
                self.emit_instruction(Opcode::BooleanNot, NwType::Integer.auxcode());
                Ok(NwType::Integer)
            }
            Operation::OnesComplement => {
                self.generate_expression(node.left)?;
                self.emit_instruction(Opcode::OnesComplement, NwType::Integer.auxcode());
                Ok(NwType::Integer)
            }
            Operation::Variable => {
                // TODO: look up variable in symbol table and emit RUNSTACK_COPY
                Ok(NwType::Integer)
            }
            Operation::Action => {
                // TODO: function call codegen
                Ok(NwType::Void)
            }
            _ => {
                if node.left != NULL_NODE {
                    self.generate_expression(node.left)?;
                }
                if node.right != NULL_NODE {
                    self.generate_expression(node.right)?;
                }
                Ok(NwType::Void)
            }
        }
    }

    fn resolve_fixups(&mut self) {
        // TODO: resolve label references to actual byte offsets
    }
}
