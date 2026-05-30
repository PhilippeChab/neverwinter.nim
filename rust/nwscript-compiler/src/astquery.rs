use serde::Serialize;

use crate::ast::{AstArena, AstNode, NodeId, NULL_NODE, Operation};
use crate::semcheck::{FunctionSig, StructDef};

#[derive(Serialize, Clone)]
pub struct AstJson {
    pub version: u32,
    pub ast: Option<AstNodeJson>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AstNodeJson {
    pub operation: String,
    pub operation_id: u32,
    pub position: AstPosition,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub string_data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integer_data: Option<Vec<i32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub float_data: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_data: Option<Vec<f32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "type")]
    pub type_str: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack_pointer: Option<i32>,
    pub left: Option<Box<AstNodeJson>>,
    pub right: Option<Box<AstNodeJson>>,
}

#[derive(Serialize, Clone)]
pub struct AstPosition {
    pub file: u32,
    pub line: u32,
    pub char: u32,
}

pub fn ast_to_json(arena: &AstArena, root: NodeId) -> AstJson {
    AstJson {
        version: 1,
        ast: node_to_json(arena, root),
    }
}

/// Cap on the emitted JSON nesting depth (both left-descent and right-spine length).
/// Two reasons: (1) `make_node_json` still recurses into `left` (deep field chains /
/// operator spines would overflow the build), and (2) `serde_json` serializes the
/// resulting `left`/`right` Box tree RECURSIVELY, so a deep structure overflows the
/// 1 MB WASM stack during serialization even though we build the right spine
/// iteratively. Beyond the cap we emit a single "(truncated)" placeholder. Real LSP
/// files are far shallower than this; only pathological or spec-sized input truncates.
const MAX_JSON_DEPTH: u32 = 1500;

fn node_to_json(arena: &AstArena, node_id: NodeId) -> Option<AstNodeJson> {
    node_to_json_d(arena, node_id, 0)
}

fn truncated_marker() -> AstNodeJson {
    AstNodeJson {
        operation: "TRUNCATED".to_string(),
        operation_id: 0,
        position: AstPosition { file: 0, line: 0, char: 0 },
        string_data: None,
        integer_data: None,
        float_data: None,
        vector_data: None,
        type_str: None,
        type_id: None,
        type_name: None,
        stack_pointer: None,
        left: None,
        right: None,
    }
}

fn node_to_json_d(arena: &AstArena, node_id: NodeId, depth: u32) -> Option<AstNodeJson> {
    if node_id == NULL_NODE { return None; }
    if depth >= MAX_JSON_DEPTH { return Some(truncated_marker()); }
    // Build the head node, then walk the right-linked spine ITERATIVELY, recursing
    // only into each node's `left`. Right spines are the long chains (StatementList,
    // FunctionalUnit, VariableList). Each right link costs one serde frame, so it
    // counts against the depth cap alongside left-descent.
    let mut head = make_node_json(arena, node_id, depth);
    let mut tail = &mut head;
    let mut rid = arena.get(node_id).right;
    let mut d = depth;
    while rid != NULL_NODE {
        d += 1;
        if d >= MAX_JSON_DEPTH {
            tail.right = Some(Box::new(truncated_marker()));
            break;
        }
        tail.right = Some(Box::new(make_node_json(arena, rid, d)));
        tail = tail.right.as_mut().unwrap();
        rid = arena.get(rid).right;
    }
    Some(head)
}

/// Build one node's JSON: own fields + recursive `left`, with `right` left empty
/// (the caller links the right-spine iteratively).
fn make_node_json(arena: &AstArena, node_id: NodeId, depth: u32) -> AstNodeJson {
    let node = arena.get(node_id);

    let op_name = operation_name(node.op);
    let op_id = operation_id(node.op);

    let integer_data = if node.int_data.iter().any(|&v| v != 0) {
        Some(node.int_data.to_vec())
    } else {
        None
    };

    let float_data = if node.float_data != 0.0 { Some(node.float_data) } else { None };

    let vector_data = if node.vector_data.iter().any(|&v| v != 0.0) {
        Some(node.vector_data.to_vec())
    } else {
        None
    };

    let stack_pointer = if node.stack_pointer != 0 { Some(node.stack_pointer) } else { None };

    AstNodeJson {
        operation: op_name.to_string(),
        operation_id: op_id,
        position: AstPosition {
            file: node.file_id,
            line: node.line,
            char: node.col,
        },
        string_data: node.string_data.clone(),
        integer_data,
        float_data,
        vector_data,
        type_str: type_name_from_nwtype(node.nw_type),
        type_id: Some(nwtype_to_id(node.nw_type)),
        type_name: node.type_name.clone(),
        stack_pointer,
        left: node_to_json_d(arena, node.left, depth + 1).map(Box::new),
        right: None,
    }
}

// ===== Position queries =====

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NodeAtPosition {
    pub operation: String,
    pub string_data: Option<String>,
    pub type_name: Option<String>,
    pub line: u32,
    pub char: u32,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionLocation {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub char: u32,
    pub return_type: Option<String>,
    pub params: Option<Vec<ParamJson>>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ParamJson {
    pub name: String,
    pub param_type: String,
    pub has_default: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CompletionInfo {
    pub kind: String,
    pub name: String,
    pub detail: Option<String>,
}

pub struct PositionQuery<'a> {
    arena: &'a AstArena,
    root: NodeId,
    functions: &'a [FunctionSig],
    structs: &'a [StructDef],
    file_names: &'a [String],
}

impl<'a> PositionQuery<'a> {
    pub fn new(
        arena: &'a AstArena,
        root: NodeId,
        functions: &'a [FunctionSig],
        structs: &'a [StructDef],
        file_names: &'a [String],
    ) -> Self {
        Self { arena, root, functions, structs, file_names }
    }

    pub fn find_node_at_position(&self, line: u32, col: u32) -> Option<NodeAtPosition> {
        let ast_line = line + 1;
        let ast_col = col + 1;

        let mut best: Option<(NodeAtPosition, u32, i64)> = None;

        self.walk(self.root, 0, &mut |node, depth| {
            if node.line == 0 { return; }
            let before = node.line < ast_line || (node.line == ast_line && node.col <= ast_col);
            if !before { return; }

            let line_dist = (node.line as i64 - ast_line as i64).unsigned_abs();
            let char_dist = if node.line == ast_line {
                (node.col as i64 - ast_col as i64).unsigned_abs()
            } else {
                1000
            };
            let dist = (line_dist * 1000 + char_dist) as i64;

            let is_better = match &best {
                None => true,
                Some((_, bd, bdist)) => {
                    if dist == 0 && *bdist != 0 { true }
                    else if *bdist == 0 && dist != 0 { false }
                    else if depth > *bd { true }
                    else if depth == *bd && node.string_data.is_some() { true }
                    else if depth == *bd && dist < *bdist { true }
                    else { false }
                }
            };

            if is_better {
                best = Some((NodeAtPosition {
                    operation: operation_name(node.op).to_string(),
                    string_data: node.string_data.clone(),
                    type_name: node.type_name.clone(),
                    line: node.line,
                    char: node.col,
                }, depth, dist));
            }
        });

        best.map(|(n, _, _)| n)
    }

    pub fn get_definition_at_position(&self, line: u32, col: u32) -> Option<DefinitionLocation> {
        let node = self.find_node_at_position(line, col)?;
        let name = node.string_data.as_deref()?;

        // Check functions
        if let Some(func) = self.functions.iter().find(|f| f.name == name) {
            let file = if !func.is_engine_action {
                self.file_names.first().cloned().unwrap_or_default()
            } else {
                "nwscript.nss".to_string()
            };
            return Some(DefinitionLocation {
                name: func.name.clone(),
                kind: "function".to_string(),
                file,
                line: 0,
                char: 0,
                return_type: Some(type_name_from_nwtype(func.return_type).unwrap_or_default()),
                params: Some(func.params.iter().map(|p| ParamJson {
                    name: p.name.clone(),
                    param_type: type_name_from_nwtype(p.nw_type).unwrap_or_default(),
                    has_default: p.has_default,
                }).collect()),
            });
        }

        // Check structs
        if let Some(sd) = self.structs.iter().find(|s| s.name == name) {
            return Some(DefinitionLocation {
                name: sd.name.clone(),
                kind: "struct".to_string(),
                file: self.file_names.first().cloned().unwrap_or_default(),
                line: 0,
                char: 0,
                return_type: None,
                params: None,
            });
        }

        None
    }

    pub fn is_in_function_call(&self, line: u32, col: u32) -> bool {
        self.find_ancestor_action(line, col).is_some()
    }

    pub fn get_function_name_at_position(&self, line: u32, col: u32) -> Option<String> {
        let action = self.find_ancestor_action(line, col)?;
        let node = self.arena.get(action);
        if node.left != NULL_NODE {
            let aid = self.arena.get(node.left);
            if aid.op == Operation::ActionId {
                return aid.string_data.clone();
            }
        }
        None
    }

    pub fn get_active_parameter_index(&self, line: u32, col: u32) -> u32 {
        let ast_line = line + 1;
        let ast_col = col + 1;

        let action = match self.find_ancestor_action(line, col) {
            Some(a) => a,
            None => return 0,
        };

        let node = self.arena.get(action);
        if node.left == NULL_NODE { return 0; }

        let aid = self.arena.get(node.left);
        let mut count = 0u32;
        let mut arg = aid.right;
        while arg != NULL_NODE {
            let arg_node = self.arena.get(arg);
            if arg_node.op == Operation::ActionArgList {
                if arg_node.line < ast_line || (arg_node.line == ast_line && arg_node.col < ast_col) {
                    count += 1;
                }
            }
            arg = arg_node.right;
        }
        if count > 0 { count - 1 } else { 0 }
    }

    pub fn get_completions_at_position(&self, line: u32, _col: u32) -> Vec<CompletionInfo> {
        let mut completions = Vec::new();

        // Add all functions
        for func in self.functions {
            let detail = format!(
                "{} {}({})",
                type_name_from_nwtype(func.return_type).unwrap_or_default(),
                func.name,
                func.params.iter()
                    .map(|p| format!("{} {}", type_name_from_nwtype(p.nw_type).unwrap_or_default(), p.name))
                    .collect::<Vec<_>>().join(", ")
            );
            completions.push(CompletionInfo {
                kind: "function".to_string(),
                name: func.name.clone(),
                detail: Some(detail),
            });
        }

        // Add all structs
        for sd in self.structs {
            completions.push(CompletionInfo {
                kind: "struct".to_string(),
                name: sd.name.clone(),
                detail: Some(format!("struct {} ({} fields)", sd.name, sd.fields.len())),
            });
        }

        // Add local variables visible at this line
        self.collect_visible_vars(self.root, line + 1, &mut completions);

        completions
    }

    fn collect_visible_vars(&self, node_id: NodeId, target_line: u32, completions: &mut Vec<CompletionInfo>) {
        // Explicit-stack pre-order walk (recursion overflows on long right-linked
        // chains — ~497 nodes — which is the common LSP-completion case).
        let mut stack = vec![node_id];
        while let Some(id) = stack.pop() {
            if id == NULL_NODE { continue; }
            let node = self.arena.get(id);
            if node.op == Operation::Variable && node.string_data.is_some() && node.line <= target_line {
                if let Some(name) = &node.string_data {
                    if !completions.iter().any(|c| c.name == *name) {
                        completions.push(CompletionInfo {
                            kind: "variable".to_string(),
                            name: name.clone(),
                            detail: node.type_name.clone(),
                        });
                    }
                }
            }
            // Push right then left so left is processed first (pre-order).
            if node.right != NULL_NODE { stack.push(node.right); }
            if node.left != NULL_NODE { stack.push(node.left); }
        }
    }

    fn find_ancestor_action(&self, line: u32, col: u32) -> Option<NodeId> {
        let ast_line = line + 1;
        let ast_col = col + 1;

        let mut result = None;
        self.find_action_containing(self.root, ast_line, ast_col, &mut result);
        result
    }

    fn find_action_containing(&self, node_id: NodeId, line: u32, col: u32, result: &mut Option<NodeId>) {
        let _ = col;
        // Explicit-stack pre-order walk. The result is the last pre-order Action with
        // line <= target — identical to the recursive node/left/right order.
        let mut stack = vec![node_id];
        while let Some(id) = stack.pop() {
            if id == NULL_NODE { continue; }
            let node = self.arena.get(id);
            if node.op == Operation::Action && node.line <= line {
                *result = Some(id);
            }
            if node.right != NULL_NODE { stack.push(node.right); }
            if node.left != NULL_NODE { stack.push(node.left); }
        }
    }

    fn walk(&self, node_id: NodeId, depth: u32, f: &mut impl FnMut(&AstNode, u32)) {
        // Explicit-stack pre-order walk preserving per-node depth (the position-query
        // tie-breaker depends on it). Recursion overflowed on long right-spines.
        let mut stack: Vec<(NodeId, u32)> = vec![(node_id, depth)];
        while let Some((id, d)) = stack.pop() {
            if id == NULL_NODE { continue; }
            let node = self.arena.get(id);
            f(node, d);
            // Push right then left so left is visited first (matches node/left/right).
            if node.right != NULL_NODE { stack.push((node.right, d + 1)); }
            if node.left != NULL_NODE { stack.push((node.left, d + 1)); }
        }
    }
}

fn operation_name(op: Operation) -> &'static str {
    match op {
        Operation::CompoundStatement => "COMPOUND_STATEMENT",
        Operation::Statement => "STATEMENT",
        Operation::StatementNoDebug => "STATEMENT_NO_DEBUG",
        Operation::KeywordDeclaration => "KEYWORD_DECLARATION",
        Operation::ConstDeclaration => "CONST_DECLARATION",
        Operation::KeywordInt => "KEYWORD_INT",
        Operation::KeywordFloat => "KEYWORD_FLOAT",
        Operation::KeywordString => "KEYWORD_STRING",
        Operation::KeywordObject => "KEYWORD_OBJECT",
        Operation::KeywordVoid => "KEYWORD_VOID",
        Operation::KeywordVector => "KEYWORD_VECTOR",
        Operation::KeywordStruct => "KEYWORD_STRUCT",
        Operation::KeywordEngineStructure(_) => "KEYWORD_ENGINE_STRUCTURE",
        Operation::VariableList => "VARIABLE_LIST",
        Operation::Variable => "VARIABLE",
        Operation::StatementList => "STATEMENT_LIST",
        Operation::IfBlock => "IF_BLOCK",
        Operation::IfChoice => "IF_CHOICE",
        Operation::IfCondition => "IF_CONDITION",
        Operation::WhileBlock => "WHILE_BLOCK",
        Operation::WhileChoice => "WHILE_CHOICE",
        Operation::WhileCondition => "WHILE_CONDITION",
        Operation::WhileContinue => "WHILE_CONTINUE",
        Operation::DoWhileBlock => "DOWHILE_BLOCK",
        Operation::DoWhileCondition => "DOWHILE_CONDITION",
        Operation::ForBlock => "FOR_BLOCK",
        Operation::SwitchBlock => "SWITCH_BLOCK",
        Operation::SwitchCondition => "SWITCH_CONDITION",
        Operation::Default => "DEFAULT",
        Operation::Case => "CASE",
        Operation::Break => "BREAK",
        Operation::Continue => "CONTINUE",
        Operation::CondBlock => "COND_BLOCK",
        Operation::CondChoice => "COND_CHOICE",
        Operation::CondCondition => "COND_CONDITION",
        Operation::FunctionalUnit => "FUNCTIONAL_UNIT",
        Operation::Function => "FUNCTION",
        Operation::FunctionIdentifier => "FUNCTION_IDENTIFIER",
        Operation::FunctionDeclaration => "FUNCTION_DECLARATION",
        Operation::FunctionParamName => "FUNCTION_PARAM_NAME",
        Operation::Action => "ACTION",
        Operation::ActionId => "ACTION_ID",
        Operation::ActionParameter => "ACTION_PARAMETER",
        Operation::ActionArgList => "ACTION_ARG_LIST",
        Operation::Return => "RETURN",
        Operation::Assignment => "ASSIGNMENT",
        Operation::LogicalOr => "LOGICAL_OR",
        Operation::LogicalAnd => "LOGICAL_AND",
        Operation::InclusiveOr => "INCLUSIVE_OR",
        Operation::ExclusiveOr => "EXCLUSIVE_OR",
        Operation::BooleanAnd => "BOOLEAN_AND",
        Operation::BooleanNot => "BOOLEAN_NOT",
        Operation::ConditionEqual => "CONDITION_EQUAL",
        Operation::ConditionNotEqual => "CONDITION_NOT_EQUAL",
        Operation::ConditionGEQ => "CONDITION_GEQ",
        Operation::ConditionGT => "CONDITION_GT",
        Operation::ConditionLT => "CONDITION_LT",
        Operation::ConditionLEQ => "CONDITION_LEQ",
        Operation::ShiftLeft => "SHIFT_LEFT",
        Operation::ShiftRight => "SHIFT_RIGHT",
        Operation::UnsignedShiftRight => "UNSIGNED_SHIFT_RIGHT",
        Operation::Add => "ADD",
        Operation::Subtract => "SUBTRACT",
        Operation::Multiply => "MULTIPLY",
        Operation::Divide => "DIVIDE",
        Operation::Modulus => "MODULUS",
        Operation::Negation => "NEGATION",
        Operation::OnesComplement => "ONES_COMPLEMENT",
        Operation::PostIncrement => "POST_INCREMENT",
        Operation::PostDecrement => "POST_DECREMENT",
        Operation::PreIncrement => "PRE_INCREMENT",
        Operation::PreDecrement => "PRE_DECREMENT",
        Operation::ConstantInteger => "CONSTANT_INTEGER",
        Operation::ConstantFloat => "CONSTANT_FLOAT",
        Operation::ConstantString => "CONSTANT_STRING",
        Operation::ConstantObject => "CONSTANT_OBJECT",
        Operation::ConstantVector => "CONSTANT_VECTOR",
        Operation::ConstantJson => "CONSTANT_JSON",
        Operation::ConstantLocation => "CONSTANT_LOCATION",
        Operation::IntegerExpression => "INTEGER_EXPRESSION",
        Operation::NonVoidExpression => "NON_VOID_EXPRESSION",
        Operation::StructureDefinition => "STRUCTURE_DEFINITION",
        Operation::StructurePart => "STRUCTURE_PART",
        Operation::GlobalVariables => "GLOBAL_VARIABLES",
    }
}

fn operation_id(op: Operation) -> u32 {
    match op {
        Operation::CompoundStatement => 0,
        Operation::Statement => 1,
        Operation::KeywordDeclaration => 2,
        Operation::KeywordInt => 3,
        Operation::KeywordFloat => 4,
        Operation::KeywordString => 5,
        Operation::KeywordObject => 6,
        Operation::VariableList => 7,
        Operation::Variable => 8,
        Operation::StatementList => 9,
        Operation::IfBlock => 10,
        Operation::IfChoice => 11,
        Operation::IfCondition => 12,
        Operation::Action => 13,
        Operation::ActionId => 14,
        Operation::Assignment => 15,
        Operation::ActionArgList => 16,
        Operation::ConstantInteger => 17,
        Operation::ConstantFloat => 18,
        Operation::ConstantString => 19,
        Operation::FunctionalUnit => 50,
        Operation::KeywordStruct => 51,
        Operation::StructureDefinition => 52,
        Operation::FunctionIdentifier => 53,
        Operation::FunctionDeclaration => 54,
        Operation::Function => 55,
        Operation::FunctionParamName => 56,
        Operation::KeywordVoid => 57,
        Operation::Return => 58,
        Operation::GlobalVariables => 73,
        Operation::SwitchBlock => 81,
        Operation::ForBlock => 89,
        Operation::ConstDeclaration => 90,
        _ => 99,
    }
}

fn nwtype_to_id(t: crate::types::NwType) -> u32 {
    match t {
        crate::types::NwType::Void => 0,
        crate::types::NwType::Integer => 1,
        crate::types::NwType::Float => 2,
        crate::types::NwType::String => 3,
        crate::types::NwType::Object => 4,
        crate::types::NwType::Vector => 5,
        crate::types::NwType::Action => 6,
        crate::types::NwType::EngineStructure(n) => 10 + n as u32,
        crate::types::NwType::Struct => 20,
    }
}

fn type_name_from_nwtype(t: crate::types::NwType) -> Option<String> {
    Some(match t {
        crate::types::NwType::Void => "void".to_string(),
        crate::types::NwType::Integer => "int".to_string(),
        crate::types::NwType::Float => "float".to_string(),
        crate::types::NwType::String => "string".to_string(),
        crate::types::NwType::Object => "object".to_string(),
        crate::types::NwType::Vector => "vector".to_string(),
        crate::types::NwType::Action => "action".to_string(),
        crate::types::NwType::EngineStructure(n) => format!("engine_structure_{}", n),
        crate::types::NwType::Struct => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::semcheck::SemanticChecker;

    fn parse_and_query(src: &str) -> (AstArena, NodeId, Vec<FunctionSig>, Vec<StructDef>, Vec<String>) {
        let src_leaked: &'static str = Box::leak(src.to_string().into_boxed_str());
        let mut lexer = Lexer::new(src_leaked, "test.nss", 0);
        let tokens = lexer.tokenize();
        let mut parser = Parser::new(tokens);
        parser.file_names.push("test.nss".to_string());
        let root = parser.parse_program().unwrap();

        // Run semantic check on the parser's arena, then take the results
        let (fns, structs) = {
            let mut checker = SemanticChecker::new(&parser.arena, &parser.file_names);
            checker.set_require_entry_point(false);
            let _ = checker.check(root);
            (checker.functions.clone(), checker.structs.clone())
        };

        let arena = std::mem::replace(&mut parser.arena, AstArena::new());
        let files = parser.file_names.clone();
        (arena, root, fns, structs, files)
    }

    #[test]
    fn test_ast_to_json() {
        let (arena, root, _, _, _) = parse_and_query("void main() { int x = 42; }");
        let json = ast_to_json(&arena, root);
        let text = serde_json::to_string(&json).unwrap();
        assert!(text.contains("FUNCTIONAL_UNIT"));
        assert!(text.contains("FUNCTION"));
        assert!(text.contains("main"));
        assert!(text.contains("CONSTANT_INTEGER"));
    }

    #[test]
    fn test_find_node_at_position() {
        let (arena, root, fns, structs, files) = parse_and_query("void main() { int x = 42; }");
        let q = PositionQuery::new(&arena, root, &fns, &structs, &files);
        let node = q.find_node_at_position(0, 5);
        assert!(node.is_some());
        let n = node.unwrap();
        assert_eq!(n.string_data.as_deref(), Some("main"));
    }

    #[test]
    fn test_get_definition() {
        let (arena, root, fns, structs, files) = parse_and_query(
            "int helper(int n) { return n; }\nvoid main() { int x = helper(1); }"
        );
        let q = PositionQuery::new(&arena, root, &fns, &structs, &files);
        let def = q.get_definition_at_position(1, 22);
        assert!(def.is_some());
        let d = def.unwrap();
        assert_eq!(d.name, "helper");
        assert_eq!(d.kind, "function");
    }

    #[test]
    fn test_is_in_function_call() {
        let (arena, root, fns, structs, files) = parse_and_query(
            "void foo(int a) { }\nvoid main() { foo(42); }"
        );
        let q = PositionQuery::new(&arena, root, &fns, &structs, &files);
        assert!(q.is_in_function_call(1, 18));
    }

    #[test]
    fn test_get_function_name() {
        let (arena, root, fns, structs, files) = parse_and_query(
            "void foo(int a) { }\nvoid main() { foo(42); }"
        );
        let q = PositionQuery::new(&arena, root, &fns, &structs, &files);
        let name = q.get_function_name_at_position(1, 18);
        assert_eq!(name.as_deref(), Some("foo"));
    }

    #[test]
    fn test_completions() {
        let (arena, root, fns, structs, files) = parse_and_query(
            "struct Vec2 { int x; int y; };\nint helper(int n) { return n; }\nvoid main() { }"
        );
        let q = PositionQuery::new(&arena, root, &fns, &structs, &files);
        let comps = q.get_completions_at_position(2, 14);
        assert!(comps.iter().any(|c| c.name == "helper" && c.kind == "function"));
        assert!(comps.iter().any(|c| c.name == "Vec2" && c.kind == "struct"));
    }

    #[test]
    fn test_struct_definition_lookup() {
        let (arena, root, fns, structs, files) = parse_and_query(
            "struct MyData { int value; float weight; };"
        );
        let q = PositionQuery::new(&arena, root, &fns, &structs, &files);
        let def = q.get_definition_at_position(0, 7);
        assert!(def.is_some());
        assert_eq!(def.unwrap().kind, "struct");
    }
}
