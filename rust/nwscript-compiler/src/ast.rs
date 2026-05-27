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
}

impl Default for AstArena {
    fn default() -> Self {
        Self::new()
    }
}
