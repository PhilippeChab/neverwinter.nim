use crate::types::NwType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    CompoundStatement,
    Statement,
    StatementNoDebug,
    KeywordDeclaration,
    ConstDeclaration,
    KeywordInt,
    KeywordFloat,
    KeywordString,
    KeywordObject,
    KeywordVoid,
    KeywordVector,
    KeywordStruct,
    KeywordEngineStructure(u8),
    VariableList,
    Variable,
    StatementList,
    // Control flow
    IfBlock,
    IfChoice,
    IfCondition,
    WhileBlock,
    WhileChoice,
    WhileCondition,
    WhileContinue,
    DoWhileBlock,
    DoWhileCondition,
    ForBlock,
    SwitchBlock,
    SwitchCondition,
    Default,
    Case,
    Break,
    Continue,
    CondBlock,
    CondChoice,
    CondCondition,
    // Functions
    FunctionalUnit,
    Function,
    FunctionIdentifier,
    FunctionDeclaration,
    FunctionParamName,
    Action,
    ActionId,
    ActionParameter,
    ActionArgList,
    Return,
    // Expressions
    Assignment,
    LogicalOr,
    LogicalAnd,
    InclusiveOr,
    ExclusiveOr,
    BooleanAnd,
    BooleanNot,
    ConditionEqual,
    ConditionNotEqual,
    ConditionGEQ,
    ConditionGT,
    ConditionLT,
    ConditionLEQ,
    ShiftLeft,
    ShiftRight,
    UnsignedShiftRight,
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulus,
    Negation,
    OnesComplement,
    PostIncrement,
    PostDecrement,
    PreIncrement,
    PreDecrement,
    // Literals
    ConstantInteger,
    ConstantFloat,
    ConstantString,
    ConstantObject,
    ConstantVector,
    ConstantJson,
    ConstantLocation,
    IntegerExpression,
    NonVoidExpression,
    // Struct
    StructureDefinition,
    StructurePart,
    // Global
    GlobalVariables,
}

pub type NodeId = u32;

pub const NULL_NODE: NodeId = u32::MAX;

#[derive(Debug, Clone)]
pub struct AstNode {
    pub op: Operation,
    pub nw_type: NwType,
    pub type_name: Option<String>,
    pub string_data: Option<String>,
    pub int_data: [i32; 4],
    pub float_data: f32,
    pub vector_data: [f32; 3],
    pub left: NodeId,
    pub right: NodeId,
    pub file_id: u32,
    pub line: u32,
    pub col: u32,
    pub stack_pointer: i32,
    pub allow_as_default_value: bool,
}

impl AstNode {
    pub fn new(op: Operation) -> Self {
        Self {
            op,
            nw_type: NwType::Void,
            type_name: None,
            string_data: None,
            int_data: [0; 4],
            float_data: 0.0,
            vector_data: [0.0; 3],
            left: NULL_NODE,
            right: NULL_NODE,
            file_id: 0,
            line: 0,
            col: 0,
            stack_pointer: 0,
            allow_as_default_value: true,
        }
    }
}

#[derive(Debug)]
pub struct AstArena {
    nodes: Vec<AstNode>,
}

impl AstArena {
    pub fn new() -> Self {
        Self { nodes: Vec::with_capacity(4096) }
    }

    pub fn alloc(&mut self, node: AstNode) -> NodeId {
        let id = self.nodes.len() as NodeId;
        self.nodes.push(node);
        id
    }

    pub fn get(&self, id: NodeId) -> &AstNode {
        &self.nodes[id as usize]
    }

    pub fn get_mut(&mut self, id: NodeId) -> &mut AstNode {
        &mut self.nodes[id as usize]
    }

    pub fn try_get(&self, id: NodeId) -> Option<&AstNode> {
        if id == NULL_NODE {
            None
        } else {
            self.nodes.get(id as usize)
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn clear(&mut self) {
        self.nodes.clear();
    }

    /// C++ NWScript folds constant arithmetic/string subexpressions at parse-tree
    /// level (scriptcompcore.cpp `ConstantFoldNode`). We replicate the subset that
    /// matters for correctness: integer/float arithmetic between literals, string
    /// concatenation, and unary negation/!/~ over literals. Folding is bottom-up
    /// and rewrites nodes in place — child pointers are zeroed when collapsed.
    pub fn fold_constants(&mut self, root: NodeId) {
        if root == NULL_NODE { return; }
        let mut stack: Vec<NodeId> = vec![root];
        let mut visited: Vec<bool> = vec![false; self.nodes.len()];
        // Iterative post-order: visit children first, then fold the node.
        while let Some(&top) = stack.last() {
            let idx = top as usize;
            if idx >= visited.len() { visited.resize(idx + 1, false); }
            if !visited[idx] {
                visited[idx] = true;
                let n = self.nodes[idx].clone();
                if n.right != NULL_NODE { stack.push(n.right); }
                if n.left != NULL_NODE { stack.push(n.left); }
            } else {
                stack.pop();
                self.try_fold_node(top);
            }
        }
    }

    fn try_fold_node(&mut self, id: NodeId) {
        let n = self.nodes[id as usize].clone();
        // Binary arithmetic on literals
        let lhs = if n.left != NULL_NODE { Some(self.nodes[n.left as usize].clone()) } else { None };
        let rhs = if n.right != NULL_NODE { Some(self.nodes[n.right as usize].clone()) } else { None };

        match n.op {
            Operation::Add | Operation::Subtract | Operation::Multiply
            | Operation::Divide | Operation::Modulus => {
                let (l, r) = match (lhs.as_ref(), rhs.as_ref()) {
                    (Some(l), Some(r)) => (l, r),
                    _ => return,
                };
                // int op int
                if l.op == Operation::ConstantInteger && r.op == Operation::ConstantInteger {
                    let a = l.int_data[0];
                    let b = r.int_data[0];
                    let v = match n.op {
                        Operation::Add => a.wrapping_add(b),
                        Operation::Subtract => a.wrapping_sub(b),
                        Operation::Multiply => a.wrapping_mul(b),
                        Operation::Divide => if b == 0 { return } else { a / b },
                        Operation::Modulus => if b == 0 { return } else { a % b },
                        _ => return,
                    };
                    let m = &mut self.nodes[id as usize];
                    m.op = Operation::ConstantInteger;
                    m.nw_type = crate::types::NwType::Integer;
                    m.int_data[0] = v;
                    m.left = NULL_NODE;
                    m.right = NULL_NODE;
                    // C++ validates a default-param value's node shape BEFORE folding, so
                    // an arithmetic expression like `2 * 3` is rejected even though it
                    // folds to a constant. Mark folded operator results as not usable as a
                    // default value (unary `-literal` is handled separately and stays ok).
                    m.allow_as_default_value = false;
                    return;
                }
                // float op float (NOT mixed int/float — C++ scriptcompcore.cpp:1608
                // explicitly comments "Expressions like '3.0f + 1' are not folded";
                // the runtime emits a mixed-type arithmetic opcode instead).
                if matches!(n.op, Operation::Add | Operation::Subtract | Operation::Multiply | Operation::Divide)
                    && l.op == Operation::ConstantFloat
                    && r.op == Operation::ConstantFloat
                {
                    let a = l.float_data;
                    let b = r.float_data;
                    let v = match n.op {
                        Operation::Add => a + b,
                        Operation::Subtract => a - b,
                        Operation::Multiply => a * b,
                        Operation::Divide => a / b, // IEEE-754 → inf/NaN
                        _ => return,
                    };
                    let m = &mut self.nodes[id as usize];
                    m.op = Operation::ConstantFloat;
                    m.nw_type = crate::types::NwType::Float;
                    m.float_data = v;
                    m.left = NULL_NODE;
                    m.right = NULL_NODE;
                    m.allow_as_default_value = false;
                    return;
                }
                // String concatenation
                if n.op == Operation::Add
                    && l.op == Operation::ConstantString
                    && r.op == Operation::ConstantString
                {
                    let ls = l.string_data.as_deref().unwrap_or("");
                    let rs = r.string_data.as_deref().unwrap_or("");
                    // C++ scriptcompcore.cpp:1727-1729 refuses to fold when the combined
                    // length would overflow the i16 length prefix used in CONSTANT STRING.
                    if ls.len() + rs.len() >= 0x8000 {
                        return;
                    }
                    let combined = format!("{}{}", ls, rs);
                    let m = &mut self.nodes[id as usize];
                    m.op = Operation::ConstantString;
                    m.nw_type = crate::types::NwType::String;
                    m.string_data = Some(combined);
                    m.left = NULL_NODE;
                    m.right = NULL_NODE;
                    m.allow_as_default_value = false;
                    return;
                }
            }
            Operation::Negation => {
                if let Some(l) = lhs.as_ref() {
                    if l.op == Operation::ConstantInteger {
                        let v = l.int_data[0].wrapping_neg();
                        let m = &mut self.nodes[id as usize];
                        m.op = Operation::ConstantInteger;
                        m.nw_type = crate::types::NwType::Integer;
                        m.int_data[0] = v;
                        m.left = NULL_NODE;
                        return;
                    }
                    if l.op == Operation::ConstantFloat {
                        let v = -l.float_data;
                        let m = &mut self.nodes[id as usize];
                        m.op = Operation::ConstantFloat;
                        m.nw_type = crate::types::NwType::Float;
                        m.float_data = v;
                        m.left = NULL_NODE;
                        return;
                    }
                }
            }
            Operation::BooleanNot => {
                if let Some(l) = lhs.as_ref() {
                    if l.op == Operation::ConstantInteger {
                        let v = if l.int_data[0] == 0 { 1 } else { 0 };
                        let m = &mut self.nodes[id as usize];
                        m.op = Operation::ConstantInteger;
                        m.nw_type = crate::types::NwType::Integer;
                        m.int_data[0] = v;
                        m.left = NULL_NODE;
                        m.allow_as_default_value = false;
                        return;
                    }
                }
            }
            Operation::OnesComplement => {
                if let Some(l) = lhs.as_ref() {
                    if l.op == Operation::ConstantInteger {
                        let v = !l.int_data[0];
                        let m = &mut self.nodes[id as usize];
                        m.op = Operation::ConstantInteger;
                        m.nw_type = crate::types::NwType::Integer;
                        m.int_data[0] = v;
                        m.left = NULL_NODE;
                        m.allow_as_default_value = false;
                        return;
                    }
                }
            }
            // Bitwise / shift over integer literals.
            // NOTE: C++ ConstantFoldNode (scriptcompcore.cpp) does NOT fold
            // UnsignedShiftRight (`>>>`) — it's absent from the fold switch and falls
            // through to the runtime form. We match that omission for byte parity;
            // `>>>` over constants is emitted as CONST/CONST/USHR.
            Operation::BooleanAnd | Operation::InclusiveOr | Operation::ExclusiveOr
            | Operation::ShiftLeft | Operation::ShiftRight => {
                let (l, r) = match (lhs.as_ref(), rhs.as_ref()) {
                    (Some(l), Some(r)) => (l, r),
                    _ => return,
                };
                if l.op == Operation::ConstantInteger && r.op == Operation::ConstantInteger {
                    let a = l.int_data[0];
                    let b = r.int_data[0];
                    let v = match n.op {
                        Operation::BooleanAnd => a & b,
                        Operation::InclusiveOr => a | b,
                        Operation::ExclusiveOr => a ^ b,
                        Operation::ShiftLeft => a.wrapping_shl((b & 31) as u32),
                        Operation::ShiftRight => a.wrapping_shr((b & 31) as u32),
                        _ => return,
                    };
                    let m = &mut self.nodes[id as usize];
                    m.op = Operation::ConstantInteger;
                    m.nw_type = crate::types::NwType::Integer;
                    m.int_data[0] = v;
                    m.left = NULL_NODE;
                    m.right = NULL_NODE;
                    m.allow_as_default_value = false;
                }
            }
            // Comparisons fold to integer 0/1
            Operation::ConditionEqual | Operation::ConditionNotEqual
            | Operation::ConditionLT | Operation::ConditionLEQ
            | Operation::ConditionGT | Operation::ConditionGEQ => {
                let (l, r) = match (lhs.as_ref(), rhs.as_ref()) {
                    (Some(l), Some(r)) => (l, r),
                    _ => return,
                };
                let mut result: Option<i32> = None;
                if l.op == Operation::ConstantInteger && r.op == Operation::ConstantInteger {
                    let a = l.int_data[0];
                    let b = r.int_data[0];
                    result = Some(match n.op {
                        Operation::ConditionEqual => (a == b) as i32,
                        Operation::ConditionNotEqual => (a != b) as i32,
                        Operation::ConditionLT => (a < b) as i32,
                        Operation::ConditionLEQ => (a <= b) as i32,
                        Operation::ConditionGT => (a > b) as i32,
                        Operation::ConditionGEQ => (a >= b) as i32,
                        _ => return,
                    });
                } else if l.op == Operation::ConstantFloat && r.op == Operation::ConstantFloat {
                    // Only fold same-type float comparisons; mixed int/float must
                    // reach semcheck which rejects them per C++.
                    let a = l.float_data;
                    let b = r.float_data;
                    result = Some(match n.op {
                        Operation::ConditionEqual => (a == b) as i32,
                        Operation::ConditionNotEqual => (a != b) as i32,
                        Operation::ConditionLT => (a < b) as i32,
                        Operation::ConditionLEQ => (a <= b) as i32,
                        Operation::ConditionGT => (a > b) as i32,
                        Operation::ConditionGEQ => (a >= b) as i32,
                        _ => return,
                    });
                } else if l.op == Operation::ConstantString && r.op == Operation::ConstantString {
                    let sa = l.string_data.as_deref().unwrap_or("");
                    let sb = r.string_data.as_deref().unwrap_or("");
                    result = match n.op {
                        Operation::ConditionEqual => Some((sa == sb) as i32),
                        Operation::ConditionNotEqual => Some((sa != sb) as i32),
                        _ => None,
                    };
                }
                if let Some(v) = result {
                    let m = &mut self.nodes[id as usize];
                    m.op = Operation::ConstantInteger;
                    m.nw_type = crate::types::NwType::Integer;
                    m.int_data[0] = v;
                    m.left = NULL_NODE;
                    m.right = NULL_NODE;
                    m.allow_as_default_value = false;
                }
            }
            // Logical && / || — fold when both operands are integer literals.
            // (Short-circuit codegen handles the common runtime path.)
            Operation::LogicalAnd | Operation::LogicalOr => {
                let (l, r) = match (lhs.as_ref(), rhs.as_ref()) {
                    (Some(l), Some(r)) => (l, r),
                    _ => return,
                };
                if l.op == Operation::ConstantInteger && r.op == Operation::ConstantInteger {
                    let a = l.int_data[0] != 0;
                    let b = r.int_data[0] != 0;
                    let v = match n.op {
                        Operation::LogicalAnd => (a && b) as i32,
                        Operation::LogicalOr => (a || b) as i32,
                        _ => return,
                    };
                    let m = &mut self.nodes[id as usize];
                    m.op = Operation::ConstantInteger;
                    m.nw_type = crate::types::NwType::Integer;
                    m.int_data[0] = v;
                    m.left = NULL_NODE;
                    m.right = NULL_NODE;
                    m.allow_as_default_value = false;
                }
            }
            _ => {}
        }
    }
}

fn literal_to_float(n: &AstNode) -> Option<f32> {
    match n.op {
        Operation::ConstantInteger => Some(n.int_data[0] as f32),
        Operation::ConstantFloat => Some(n.float_data),
        _ => None,
    }
}

impl Default for AstArena {
    fn default() -> Self {
        Self::new()
    }
}
